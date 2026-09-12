import { useState, type ReactElement } from 'react'
import type { PluginCommand, PluginPermissionDecision, PluginSettingsField } from '@plugin-api/shared'

const PERMISSION_OPTIONS: Array<{ value: PluginPermissionDecision; label: string }> = [
  { value: 'allow', label: 'Allow' },
  { value: 'ask', label: 'Ask' },
  { value: 'deny', label: 'Deny' }
]

function PermissionControl({
  value,
  onChange
}: {
  value: unknown
  onChange: (value: PluginPermissionDecision) => void
}): ReactElement {
  const current: PluginPermissionDecision =
    value === 'allow' || value === 'deny' || value === 'ask' ? value : 'allow'
  return (
    <div className="plugin-settings-permission" role="group" aria-label="Permission">
      {PERMISSION_OPTIONS.map((opt) => (
        <button
          key={opt.value}
          type="button"
          className={`plugin-settings-permission-btn plugin-settings-permission-${opt.value}${
            current === opt.value ? ' is-active' : ''
          }`}
          aria-pressed={current === opt.value}
          onClick={() => onChange(opt.value)}
        >
          {opt.label}
        </button>
      ))}
    </div>
  )
}

function MacroListEditor({
  value,
  onChange
}: {
  value: PluginCommand[]
  onChange: (next: PluginCommand[]) => void
}): ReactElement {
  return (
    <div className="plugin-settings-macros">
      {value.map((btn, index) => (
        <div key={btn.id} className="plugin-settings-macro-row">
          <input
            aria-label="Label"
            placeholder="Label"
            value={btn.label}
            onChange={(e) => {
              const next = value.slice()
              next[index] = { ...btn, label: e.target.value }
              onChange(next)
            }}
          />
          <input
            aria-label="Text to send"
            placeholder="Text to send"
            value={btn.text}
            onChange={(e) => {
              const next = value.slice()
              next[index] = { ...btn, text: e.target.value }
              onChange(next)
            }}
          />
          <input
            aria-label="Hotkey"
            placeholder="Hotkey"
            value={btn.hotkey}
            onChange={(e) => {
              const next = value.slice()
              next[index] = { ...btn, hotkey: e.target.value }
              onChange(next)
            }}
          />
          <button
            type="button"
            onClick={() => onChange(value.filter((_, i) => i !== index))}
          >
            Remove
          </button>
        </div>
      ))}
      <button
        type="button"
        onClick={() =>
          onChange([
            ...value,
            {
              id: crypto.randomUUID(),
              label: 'New',
              text: '',
              hotkey: ''
            }
          ])
        }
      >
        Add button
      </button>
    </div>
  )
}

function ItemListEditor({
  field,
  value,
  onChange,
  onAction
}: {
  field: PluginSettingsField
  value: unknown
  onChange: (value: unknown) => void
  onAction?: () => void
}): ReactElement {
  if (field.type === 'action') {
    return (
      <button type="button" className="primary" onClick={onAction}>
        {field.label}
      </button>
    )
  }
  const items = Array.isArray(value) ? value : []
  const schema = field.itemSchema ?? []
  const [expandedItems, setExpandedItems] = useState<Set<number>>(() => new Set())
  const newItem = (): Record<string, unknown> =>
    Object.fromEntries(schema.map((child) => [child.key, child.default]))
  const updateItem = (index: number, key: string, nextValue: unknown): void => {
    const next = items.map((item, itemIndex) =>
      itemIndex === index && item && typeof item === 'object' && !Array.isArray(item)
        ? { ...(item as Record<string, unknown>), [key]: nextValue }
        : item
    )
    onChange(next)
  }

  return (
    <div className="plugin-settings-items">
      {items.map((item, index) => {
        const values = item && typeof item === 'object' && !Array.isArray(item)
          ? (item as Record<string, unknown>)
          : newItem()
        const expanded = expandedItems.has(index)
        return (
          <div key={index} className="plugin-settings-item">
            <div className="plugin-settings-item-header">
              <button
                type="button"
                className="plugin-settings-item-toggle"
                aria-expanded={expanded}
                onClick={() => {
                  setExpandedItems((previous) => {
                    const next = new Set(previous)
                    if (next.has(index)) {
                      next.delete(index)
                    } else {
                      next.add(index)
                    }
                    return next
                  })
                }}
              >
                <span aria-hidden="true">{expanded ? '-' : '+'}</span>
                <strong>{field.itemLabel || 'Item'} {index + 1}</strong>
              </button>
              <button type="button" onClick={() => onChange(items.filter((_, i) => i !== index))}>
                Remove
              </button>
            </div>
            {expanded ? (
              <div className="plugin-settings-item-fields">
                {schema.map((child) => (
                  <div key={child.key} className="plugin-settings-item-field">
                    <div className="settings-row-label">
                      <strong>{child.label}</strong>
                      {child.description ? <span>{child.description}</span> : null}
                    </div>
                    <PluginFieldEditor
                      field={child}
                      value={values[child.key]}
                      onChange={(nextValue) => updateItem(index, child.key, nextValue)}
                    />
                  </div>
                ))}
              </div>
            ) : null}
          </div>
        )
      })}
      <button type="button" onClick={() => onChange([...items, newItem()])}>
        Add {field.itemLabel || 'item'}
      </button>
    </div>
  )
}

export default function PluginFieldEditor({
  field,
  value,
  onChange,
  onAction
}: {
  field: PluginSettingsField
  value: unknown
  onChange: (value: unknown) => void
  onAction?: () => void
}): ReactElement {
  if (field.type === 'action') {
    return (
      <button type="button" className="primary" onClick={onAction}>
        {field.label}
      </button>
    )
  }
  if (field.type === 'boolean' || field.type === 'group') {
    return (
      <input
        type="checkbox"
        checked={Boolean(value)}
        onChange={(e) => onChange(e.target.checked)}
      />
    )
  }
  if (field.type === 'number') {
    return (
      <input
        type="number"
        value={typeof value === 'number' ? value : Number(value) || 0}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    )
  }
  if (field.type === 'select') {
    const currentValue = typeof value === 'string' ? value : String(value ?? field.default ?? '')
    return (
      <select
        value={currentValue}
        onChange={(e) => onChange(e.target.value)}
      >
        {(field.options || []).map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
    )
  }
  if (field.type === 'commandList') {
    const list = Array.isArray(value) ? (value as PluginCommand[]) : []
    return <MacroListEditor value={list} onChange={onChange} />
  }
  if (field.type === 'itemList') {
    return <ItemListEditor field={field} value={value} onChange={onChange} />
  }
  if (field.type === 'permission') {
    return <PermissionControl value={value} onChange={onChange} />
  }
  if (field.type === 'stringList') {
    const text = Array.isArray(value) ? (value as string[]).join('\n') : ''
    return (
      <textarea
        rows={4}
        value={text}
        onChange={(e) =>
          onChange(
            e.target.value
              .split('\n')
              .map((l) => l.trimEnd())
              .filter((l, i, arr) => l.length > 0 || i < arr.length - 1)
          )
        }
      />
    )
  }
  if (field.type === 'textArea') {
    return (
      <textarea
        rows={6}
        value={typeof value === 'string' ? value : String(value ?? '')}
        onChange={(e) => onChange(e.target.value)}
      />
    )
  }
  if (field.type === 'directory') {
    const path = typeof value === 'string' ? value : String(value ?? '')
    return (
      <div className="plugin-settings-directory">
        <input type="text" value={path} readOnly />
        <button
          type="button"
          onClick={() => {
            void window.wassh.pickDirectory().then((selected) => {
              if (selected) {
                onChange(selected)
              }
            })
          }}
        >
          Browse
        </button>
        {path ? (
          <button type="button" onClick={() => onChange('')}>
            Clear
          </button>
        ) : null}
      </div>
    )
  }
  return (
    <input
      type={field.secret ? 'password' : 'text'}
      value={typeof value === 'string' ? value : String(value ?? '')}
      onChange={(e) => onChange(e.target.value)}
      autoComplete={field.secret ? 'off' : undefined}
    />
  )
}
