import { useCallback, useEffect, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import type { PluginMacroButton } from '@shared/plugins'
import type { PluginViewProps } from '../registry'
import ColorHexInput from '../../components/ColorHexInput'

/** Suggested formatting for the optional hotkey field */
const HOTKEY_HINT = 'e.g. Ctrl+Shift+1'
/** Separator shown when collapsing newlines in the command preview */
const PREVIEW_LINE_SEP = ' ↵ '
/** Minimum width of a macro context menu */
const CONTEXT_MENU_MIN_WIDTH_PX = 120
/** Approximate heights used to flip anchored menus above the cursor */
const ROW_MENU_HEIGHT_PX = 118
const GROUP_MENU_HEIGHT_PX = 152
const COLOR_POP_HEIGHT_PX = 176
/** Color popover width (kept in sync with .plugin-macro-color-pop) */
const COLOR_POP_WIDTH_PX = 224
/** Gap between the cursor and anchored popups */
const MENU_GAP_PX = 4
/** Space kept between anchored popups and the viewport edge */
const MENU_MARGIN_PX = 4
/** Base name for newly created macro groups */
const NEW_GROUP_BASE_NAME = 'New group'
/** Sentinal used when a macro belongs to no named group */
const UNGROUPED_GROUP_ID = ''
/** Preset accent colors offered in the group color picker */
const GROUP_COLOR_PRESETS = [
  '#e35d6a',
  '#f0b429',
  '#3dd68c',
  '#14b8a6',
  '#3d8bfd',
  '#5a9dff',
  '#8b5cf6',
  '#d946ef',
  '#ec4899',
  '#f97316',
  '#84cc16',
  '#94a3b8'
]
/** Native swatch fallback while a group has no color */
const GROUP_COLOR_FALLBACK = '#3d8bfd'
/** 6-digit hex color (#rrggbb), optional leading # */
const HEX_COLOR_RE = /^#?[0-9a-fA-F]{6}$/

interface MacroGroup {
  id: string
  name: string
  /** Accent hex for the section header; absent = no color */
  color?: string
  collapsed: boolean
}

type MacroDialogState =
  | { mode: 'add'; groupId: string }
  | { mode: 'edit'; id: string }

interface MenuPos {
  x: number
  y: number
}

function anchorAt(
  x: number,
  y: number,
  width: number,
  height: number
): { left: number; top: number } {
  const left = Math.min(x, window.innerWidth - width - MENU_MARGIN_PX)
  const below = y + MENU_GAP_PX + height
  const top =
    below <= window.innerHeight
      ? y + MENU_GAP_PX
      : Math.max(MENU_MARGIN_PX, y - MENU_GAP_PX - height)
  return { left: Math.max(MENU_MARGIN_PX, left), top }
}

function parseHotkey(hotkey: string): { ctrl: boolean; shift: boolean; alt: boolean; key: string } | null {
  const parts = hotkey
    .split('+')
    .map((p) => p.trim())
    .filter(Boolean)
  if (parts.length === 0) {
    return null
  }
  const key = parts[parts.length - 1]
  const mods = parts.slice(0, -1).map((p) => p.toLowerCase())
  return {
    ctrl: mods.includes('ctrl') || mods.includes('control') || mods.includes('cmd') || mods.includes('meta'),
    shift: mods.includes('shift'),
    alt: mods.includes('alt'),
    key: key.length === 1 ? key.toLowerCase() : key
  }
}

function matchesHotkey(e: KeyboardEvent, hotkey: string): boolean {
  const parsed = parseHotkey(hotkey)
  if (!parsed) {
    return false
  }
  const eventKey = e.key.length === 1 ? e.key.toLowerCase() : e.key
  return (
    Boolean(e.ctrlKey || e.metaKey) === parsed.ctrl &&
    e.shiftKey === parsed.shift &&
    e.altKey === parsed.alt &&
    eventKey === parsed.key
  )
}

/** Ignore key events typed into a field, not at the terminal */
function keyDownTargetsEditor(e: KeyboardEvent): boolean {
  const target = e.target as HTMLElement | null
  return Boolean(target?.closest('input, textarea, select, [contenteditable="true"]'))
}

/** Single-line preview of the (possibly multiline) macro text */
function macroPreview(text: string): string {
  const lines = text.split('\n').map((line) => line.trimEnd())
  while (lines.length > 0 && lines[lines.length - 1] === '') {
    lines.pop()
  }
  return lines.join(PREVIEW_LINE_SEP)
}

function normalizeGroupColor(raw: unknown): string | undefined {
  if (typeof raw !== 'string') {
    return undefined
  }
  const m = HEX_COLOR_RE.exec(raw.trim())
  return m ? `#${m[0].toLowerCase().replace('#', '')}` : undefined
}

/** Validate stored macro group list (defensive: data lives in app settings) */
function normalizeGroups(raw: unknown): MacroGroup[] {
  if (!Array.isArray(raw)) {
    return []
  }
  const seen = new Set<string>()
  const out: MacroGroup[] = []
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object') {
      continue
    }
    const id = (entry as { id?: unknown }).id
    if (typeof id !== 'string' || !id || seen.has(id)) {
      continue
    }
    const rawName = (entry as { name?: unknown }).name
    seen.add(id)
    out.push({
      id,
      name:
        typeof rawName === 'string' && rawName.trim() ? rawName.trim() : NEW_GROUP_BASE_NAME,
      color: normalizeGroupColor((entry as { color?: unknown }).color),
      collapsed: (entry as { collapsed?: unknown }).collapsed === true
    })
  }
  return out
}

function uniqueGroupName(groups: MacroGroup[]): string {
  const names = new Set(groups.map((g) => g.name.toLowerCase()))
  if (!names.has(NEW_GROUP_BASE_NAME.toLowerCase())) {
    return NEW_GROUP_BASE_NAME
  }
  let n = 2
  while (names.has(`${NEW_GROUP_BASE_NAME} ${n}`.toLowerCase())) {
    n += 1
  }
  return `${NEW_GROUP_BASE_NAME} ${n}`
}

/** Place `macro` at the end of its group within the stored button list */
function placedForGroup(
  buttons: PluginMacroButton[],
  macro: PluginMacroButton
): PluginMacroButton[] {
  const list = buttons.filter((b) => b.id !== macro.id)
  let index = list.length
  for (let i = list.length - 1; i >= 0; i -= 1) {
    if ((list[i].groupId || UNGROUPED_GROUP_ID) === (macro.groupId || UNGROUPED_GROUP_ID)) {
      index = i + 1
      break
    }
  }
  list.splice(index, 0, macro)
  return list
}

interface MacroEditorDialogProps {
  title: string
  macro: PluginMacroButton | null
  /** Initial group for a new macro, or the macro's current group */
  groupId: string
  groups: MacroGroup[]
  /** Other macros, used to reject duplicate hotkeys */
  siblings: PluginMacroButton[]
  onSave: (macro: PluginMacroButton) => void
  onCancel: () => void
}

function MacroEditorDialog({
  title,
  macro,
  groupId: initialGroupId,
  groups,
  siblings,
  onSave,
  onCancel
}: MacroEditorDialogProps) {
  const [label, setLabel] = useState(macro?.label ?? '')
  const [hotkey, setHotkey] = useState(macro?.hotkey ?? '')
  const [text, setText] = useState(macro?.text ?? '')
  const [groupId, setGroupId] = useState(initialGroupId)

  const trimmedLabel = label.trim()
  const trimmedHotkey = hotkey.trim()
  const duplicateHotkey = trimmedHotkey
    ? siblings.some((b) => b.hotkey.trim().toLowerCase() === trimmedHotkey.toLowerCase())
    : false
  const canSave = trimmedLabel !== '' && text.length > 0 && !duplicateHotkey

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onCancel()
      }
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [onCancel])

  const submit = (): void => {
    if (!canSave) {
      return
    }
    onSave({
      id: macro?.id ?? crypto.randomUUID(),
      label: trimmedLabel,
      text,
      hotkey: trimmedHotkey,
      groupId
    })
  }

  return createPortal(
    <div
      className="plugin-macro-dialog-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) {
          onCancel()
        }
      }}
    >
      <form
        className="plugin-macro-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onSubmit={(e) => {
          e.preventDefault()
          submit()
        }}
      >
        <div className="plugin-macro-dialog-title">{title}</div>
        <label className="plugin-macro-dialog-field">
          <span>Label</span>
          <input
            type="text"
            value={label}
            autoFocus
            placeholder="Name shown in the macro list"
            onChange={(e) => setLabel(e.target.value)}
          />
        </label>
        <label className="plugin-macro-dialog-field">
          <span>Group</span>
          <select value={groupId} onChange={(e) => setGroupId(e.target.value)}>
            <option value={UNGROUPED_GROUP_ID}>Ungrouped</option>
            {groups.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name}
              </option>
            ))}
          </select>
        </label>
        <label className="plugin-macro-dialog-field">
          <span>Hotkey (optional)</span>
          <input
            type="text"
            value={hotkey}
            placeholder={HOTKEY_HINT}
            spellCheck={false}
            onChange={(e) => setHotkey(e.target.value)}
          />
        </label>
        <label className="plugin-macro-dialog-field">
          <span>Command</span>
          <textarea
            rows={8}
            value={text}
            spellCheck={false}
            placeholder="tail -f /var/log/syslog"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                e.preventDefault()
                submit()
              }
            }}
          />
        </label>
        <div className="plugin-macro-dialog-hint">
          Multiline commands are sent exactly as typed — end with a trailing newline if it should
          run immediately. Ctrl+Enter saves.
        </div>
        {duplicateHotkey ? (
          <div className="plugin-macro-dialog-error">
            Hotkey “{trimmedHotkey}” is already used by another macro.
          </div>
        ) : null}
        <div className="plugin-macro-dialog-actions">
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
          <button type="submit" className="primary" disabled={!canSave}>
            Save
          </button>
        </div>
      </form>
    </div>,
    document.body
  )
}

export default function MacroPadView({
  tabId,
  pluginId,
  settings,
  onSettingsPatch,
  activeTab
}: PluginViewProps & { activeTab?: boolean }) {
  const storedButtons = Array.isArray(settings.buttons)
    ? (settings.buttons as PluginMacroButton[])
    : []
  const groups = normalizeGroups(settings.groups)
  const ungroupedCollapsed = settings.ungroupedCollapsed === true
  const validGroupIds = new Set(groups.map((g) => g.id))
  // A macro referencing a deleted/unknown group counts as ungrouped.
  const buttons = storedButtons.map((b) =>
    b.groupId && !validGroupIds.has(b.groupId) ? { ...b, groupId: '' } : b
  )

  const [dialog, setDialog] = useState<MacroDialogState | null>(null)
  const [rowMenu, setRowMenu] = useState<{ id: string; left: number; top: number } | null>(null)
  const [groupMenu, setGroupMenu] = useState<{ groupId: string; left: number; top: number } | null>(null)
  const [colorMenu, setColorMenu] = useState<{ groupId: string; left: number; top: number } | null>(null)
  const [renamingGroupId, setRenamingGroupId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState('')

  const send = useCallback(
    (text: string): void => {
      void window.wassh.sendPluginMessage(tabId, pluginId, { type: 'send', text })
    },
    [tabId, pluginId]
  )

  useEffect(() => {
    if (activeTab === false) {
      return
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      if (dialog || rowMenu || groupMenu || colorMenu || keyDownTargetsEditor(e)) {
        return
      }
      for (const btn of buttons) {
        if (!btn.hotkey) {
          continue
        }
        if (matchesHotkey(e, btn.hotkey)) {
          e.preventDefault()
          send(btn.text)
          return
        }
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [buttons, tabId, pluginId, activeTab, dialog, rowMenu, groupMenu, colorMenu, send])

  const closeMenus = (): void => {
    setRowMenu(null)
    setGroupMenu(null)
    setColorMenu(null)
  }

  const persistButtons = (next: PluginMacroButton[]): void => {
    onSettingsPatch({ buttons: next })
    setDialog(null)
    closeMenus()
  }

  const saveGroups = (next: MacroGroup[]): void => {
    onSettingsPatch({ groups: next })
  }

  const toggleSection = (groupId: string): void => {
    if (groupId === UNGROUPED_GROUP_ID) {
      onSettingsPatch({ ungroupedCollapsed: !ungroupedCollapsed })
      return
    }
    saveGroups(groups.map((g) => (g.id === groupId ? { ...g, collapsed: !g.collapsed } : g)))
  }

  const openRowMenu = (id: string, x: number, y: number): void => {
    setGroupMenu(null)
    setColorMenu(null)
    setRowMenu({ id, ...anchorAt(x, y, CONTEXT_MENU_MIN_WIDTH_PX, ROW_MENU_HEIGHT_PX) })
  }

  const openGroupMenu = (groupId: string, x: number, y: number): void => {
    setRowMenu(null)
    setColorMenu(null)
    setGroupMenu({
      groupId,
      ...anchorAt(x, y, CONTEXT_MENU_MIN_WIDTH_PX, GROUP_MENU_HEIGHT_PX)
    })
  }

  const openColorPop = (): void => {
    if (!groupMenu) {
      return
    }
    const { left, top } = anchorAt(
      groupMenu.left,
      groupMenu.top,
      COLOR_POP_WIDTH_PX,
      COLOR_POP_HEIGHT_PX
    )
    setRowMenu(null)
    setGroupMenu(null)
    setColorMenu({ groupId: groupMenu.groupId, left, top })
  }

  useEffect(() => {
    if (!rowMenu && !groupMenu && !colorMenu) {
      return
    }
    const close = (ev: MouseEvent): void => {
      const target = ev.target
      if (
        target instanceof Element &&
        (target.closest('.plugin-macro-context-menu') || target.closest('.plugin-macro-color-pop'))
      ) {
        return
      }
      closeMenus()
    }
    const closeOnScroll = (): void => closeMenus()
    const closeOnEscape = (ev: KeyboardEvent): void => {
      if (ev.key === 'Escape') {
        closeMenus()
      }
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('scroll', closeOnScroll, true)
    document.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('scroll', closeOnScroll, true)
      document.removeEventListener('keydown', closeOnEscape)
    }
  }, [rowMenu, groupMenu, colorMenu])

  const startRenameGroup = (groupId: string): void => {
    const group = groups.find((g) => g.id === groupId)
    if (!group) {
      return
    }
    closeMenus()
    setRenamingGroupId(groupId)
    setRenameDraft(group.name)
  }

  const commitRenameGroup = (): void => {
    if (!renamingGroupId) {
      return
    }
    const name = renameDraft.trim()
    if (name) {
      saveGroups(groups.map((g) => (g.id === renamingGroupId ? { ...g, name } : g)))
    }
    setRenamingGroupId(null)
    setRenameDraft('')
  }

  const cancelRenameGroup = (): void => {
    setRenamingGroupId(null)
    setRenameDraft('')
  }

  const addNewGroup = (): void => {
    const group: MacroGroup = {
      id: crypto.randomUUID(),
      name: uniqueGroupName(groups),
      collapsed: false
    }
    closeMenus()
    saveGroups([...groups, group])
    setRenamingGroupId(group.id)
    setRenameDraft(group.name)
  }

  const deleteGroup = (groupId: string): void => {
    onSettingsPatch({
      groups: groups.filter((g) => g.id !== groupId),
      buttons: buttons.map((b) =>
        (b.groupId || UNGROUPED_GROUP_ID) === groupId ? { ...b, groupId: '' } : b
      )
    })
    closeMenus()
  }

  const setGroupColorById = (groupId: string, color?: string): void => {
    saveGroups(
      groups.map((g) => {
        if (g.id !== groupId) {
          return g
        }
        const next = { ...g }
        if (color) {
          next.color = color
        } else {
          delete next.color
        }
        return next
      })
    )
  }

  const saveMacro = (macro: PluginMacroButton): void => {
    const editing = dialog?.mode === 'edit' ? buttons.find((b) => b.id === dialog.id) : undefined
    const base = editing ? buttons.map((b) => (b.id === macro.id ? macro : b)) : buttons
    const moved = editing ? (editing.groupId || '') !== (macro.groupId || '') : true
    persistButtons(moved ? placedForGroup(base, macro) : base)
  }

  const menuMacro = rowMenu ? (buttons.find((b) => b.id === rowMenu.id) ?? null) : null
  const menuGroup = groupMenu ? (groups.find((g) => g.id === groupMenu.groupId) ?? null) : null
  const colorGroup = colorMenu ? (groups.find((g) => g.id === colorMenu.groupId) ?? null) : null
  const dialogMacro =
    dialog?.mode === 'edit' ? (buttons.find((b) => b.id === dialog.id) ?? null) : null
  const dialogGroupId =
    dialog?.mode === 'edit'
      ? (dialogMacro?.groupId ?? '')
      : dialog?.mode === 'add'
        ? dialog.groupId
        : ''

  const sections: Array<{
    key: string
    title: string
    color?: string
    collapsed: boolean
    named: boolean
    members: PluginMacroButton[]
  }> = [
    ...groups.map((g) => ({
      key: g.id,
      title: g.name,
      color: g.color,
      collapsed: g.collapsed,
      named: true,
      members: buttons.filter((b) => (b.groupId || UNGROUPED_GROUP_ID) === g.id)
    })),
    {
      key: UNGROUPED_GROUP_ID,
      title: 'Ungrouped',
      color: undefined,
      collapsed: ungroupedCollapsed,
      named: false,
      members: buttons.filter((b) => !(b.groupId || UNGROUPED_GROUP_ID))
    }
  ]

  const hasAnyMacrosOrGroups = buttons.length > 0 || groups.length > 0

  return (
    <div
      className="plugin-panel plugin-macro-pad"
      onMouseDown={(e) => {
        // Keep terminal focus when interacting with the pad.
        if ((e.target as HTMLElement).closest('button, input, select, textarea, a')) {
          return
        }
        e.preventDefault()
      }}
    >
      <div className="plugin-macro-list">
        {!hasAnyMacrosOrGroups ? (
          <div className="plugin-macro-empty">No macros yet — use “Add macro” to define one.</div>
        ) : (
          sections.map((s) => (
            <section
              key={s.key}
              className={`plugin-macro-section${s.collapsed ? ' collapsed' : ''}`}
              data-accent={s.color || undefined}
              style={s.color ? ({ '--macro-accent': s.color } as CSSProperties) : undefined}
            >
              <div
                className="plugin-macro-section-head"
                onClick={(e) => {
                  const target = e.target as Element
                  if (
                    target.closest(
                      '.plugin-macro-section-title-input, .plugin-macro-section-collapse'
                    )
                  ) {
                    return
                  }
                  toggleSection(s.key)
                }}
                onDoubleClick={(e) => {
                  if (!s.named) {
                    return
                  }
                  e.preventDefault()
                  startRenameGroup(s.key)
                }}
                onContextMenu={(e) => {
                  if (!s.named) {
                    return
                  }
                  e.preventDefault()
                  openGroupMenu(s.key, e.clientX, e.clientY)
                }}
              >
                <button
                  type="button"
                  className="plugin-macro-section-collapse"
                  aria-label={s.collapsed ? 'Expand group' : 'Collapse group'}
                  onClick={(e) => {
                    e.stopPropagation()
                    toggleSection(s.key)
                  }}
                >
                  {s.collapsed ? '▸' : '▾'}
                </button>
                {s.color ? (
                  <span className="plugin-macro-section-dot" aria-hidden="true" />
                ) : null}
                {s.named && renamingGroupId === s.key ? (
                  <input
                    className="plugin-macro-section-title-input"
                    value={renameDraft}
                    autoFocus
                    onChange={(e) => setRenameDraft(e.target.value)}
                    onBlur={commitRenameGroup}
                    onClick={(e) => e.stopPropagation()}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        commitRenameGroup()
                      } else if (e.key === 'Escape') {
                        e.preventDefault()
                        e.stopPropagation()
                        cancelRenameGroup()
                      }
                    }}
                  />
                ) : (
                  <span className="plugin-macro-section-title">{s.title}</span>
                )}
                <span className="plugin-macro-section-count">{s.members.length}</span>
              </div>
              {!s.collapsed ? (
                <div className="plugin-macro-section-body">
                  {s.members.length === 0 ? (
                    <div className="plugin-macro-section-empty">No macros</div>
                  ) : (
                    s.members.map((btn) => (
                      <div
                        key={btn.id}
                        className="plugin-macro-row"
                        role="button"
                        tabIndex={-1}
                        aria-label={btn.label || 'Untitled'}
                        onClick={() => send(btn.text)}
                        onContextMenu={(e) => {
                          e.preventDefault()
                          openRowMenu(btn.id, e.clientX, e.clientY)
                        }}
                      >
                        <span className="plugin-macro-main">
                          <span className="plugin-macro-label">{btn.label || 'Untitled'}</span>
                          <span className="plugin-macro-text">{macroPreview(btn.text)}</span>
                        </span>
                        {btn.hotkey ? (
                          <span className="plugin-macro-hotkey">{btn.hotkey}</span>
                        ) : null}
                      </div>
                    ))
                  )}
                </div>
              ) : null}
            </section>
          ))
        )}
      </div>

      <div className="plugin-macro-footer">
        <button
          type="button"
          className="primary"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            closeMenus()
            setDialog({ mode: 'add', groupId: '' })
          }}
        >
          ＋ Add macro
        </button>
        <button type="button" onMouseDown={(e) => e.preventDefault()} onClick={addNewGroup}>
          New group
        </button>
      </div>

      {rowMenu && menuMacro ? (
        <div
          className="plugin-macro-context-menu"
          role="menu"
          style={{ left: rowMenu.left, top: rowMenu.top }}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button
            type="button"
            role="menuitem"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              send(menuMacro.text)
              closeMenus()
            }}
          >
            Run
          </button>
          <button
            type="button"
            role="menuitem"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              closeMenus()
              setDialog({ mode: 'edit', id: menuMacro.id })
            }}
          >
            Edit
          </button>
          <button
            type="button"
            role="menuitem"
            className="danger"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              closeMenus()
              persistButtons(buttons.filter((b) => b.id !== menuMacro.id))
            }}
          >
            Remove
          </button>
        </div>
      ) : null}

      {groupMenu && menuGroup ? (
        <div
          className="plugin-macro-context-menu"
          role="menu"
          style={{ left: groupMenu.left, top: groupMenu.top }}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button
            type="button"
            role="menuitem"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              setDialog({ mode: 'add', groupId: menuGroup.id })
              closeMenus()
            }}
          >
            Add macro
          </button>
          <button
            type="button"
            role="menuitem"
            onMouseDown={(e) => e.preventDefault()}
            onClick={openColorPop}
          >
            Color…
          </button>
          <button
            type="button"
            role="menuitem"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => startRenameGroup(menuGroup.id)}
          >
            Rename
          </button>
          <button
            type="button"
            role="menuitem"
            className="danger"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => deleteGroup(menuGroup.id)}
          >
            Delete
          </button>
        </div>
      ) : null}

      {colorMenu && colorGroup ? (
        <div
          className="plugin-macro-color-pop"
          role="dialog"
          aria-label={`Color for ${colorGroup.name}`}
          style={{ left: colorMenu.left, top: colorMenu.top }}
          onContextMenu={(e) => e.preventDefault()}
        >
          <div className="plugin-macro-color-title">Group color</div>
          <div className="plugin-macro-color-swatches">
            {GROUP_COLOR_PRESETS.map((c) => (
              <button
                key={c}
                type="button"
                className={`plugin-macro-color-swatch${colorGroup.color === c ? ' selected' : ''}`}
                style={{ background: c }}
                title={c}
                aria-label={c}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => setGroupColorById(colorGroup.id, c)}
              />
            ))}
          </div>
          <div className="plugin-macro-color-custom">
            <input
              type="color"
              value={colorGroup.color ?? GROUP_COLOR_FALLBACK}
              title="Custom color"
              onChange={(e) => setGroupColorById(colorGroup.id, e.target.value)}
            />
            <ColorHexInput
              value={colorGroup.color ?? ''}
              onChange={(hex) => setGroupColorById(colorGroup.id, hex || undefined)}
            />
          </div>
          {colorGroup.color ? (
            <button
              type="button"
              className="plugin-macro-color-clear"
              onClick={() => setGroupColorById(colorGroup.id, undefined)}
            >
              Remove color
            </button>
          ) : null}
        </div>
      ) : null}

      {dialog ? (
        <MacroEditorDialog
          title={dialog.mode === 'add' ? 'Add macro' : 'Edit macro'}
          macro={dialogMacro}
          groupId={dialogGroupId}
          groups={groups}
          siblings={buttons.filter((b) => b.id !== (dialog.mode === 'edit' ? dialog.id : ''))}
          onSave={saveMacro}
          onCancel={() => setDialog(null)}
        />
      ) : null}
    </div>
  )
}




