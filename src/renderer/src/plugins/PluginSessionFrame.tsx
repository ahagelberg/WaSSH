import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode
} from 'react'
import type { AppSettings } from '@shared/types'
import type { PluginListItem } from '@shared/plugins'
import { mergePluginSessionSettings } from '@shared/plugins'
import {
  INNER_DROP_BAND_PX,
  MIN_DOCK_SIZE_PX,
  OUTER_DROP_BAND_PX,
  clampSplitRatio,
  dockPluginOnEdge,
  ensurePluginInLayout,
  innerInsertZone,
  outerInsertZone,
  pruneLayoutToActive,
  setDockSplitRatio,
  splitPluginLeaf,
  type DockEdge,
  type LayoutNode,
  type LeafSplitZone,
  type TabPluginLayout
} from '@shared/pluginLayout'
import { getPluginView } from './registry'
import MacroPadView from './builtins/MacroPadView'
import PluginPanelShell from './PluginPanelShell'

interface Props {
  tabId: string
  active: boolean
  plugins: PluginListItem[]
  activePluginIds: string[]
  layout: TabPluginLayout
  settings: AppSettings
  /** Per-host plugin settings from the tab connection / host profile */
  hostPluginSettings: Record<string, Record<string, unknown>>
  onPluginSettingsPatch: (pluginId: string, partial: Record<string, unknown>) => void
  onLayoutChange: (layout: TabPluginLayout) => void
  onDeactivatePlugin: (pluginId: string) => void
  children: ReactNode
}

type DropTarget =
  | { kind: 'edge'; edge: DockEdge; insert: 'inner' | 'outer' }
  | { kind: 'leaf'; pluginId: string; zone: LeafSplitZone }

function zoneFromPoint(rect: DOMRect, clientX: number, clientY: number): LeafSplitZone {
  const x = (clientX - rect.left) / Math.max(rect.width, 1)
  const y = (clientY - rect.top) / Math.max(rect.height, 1)
  const distLeft = x
  const distRight = 1 - x
  const distTop = y
  const distBottom = 1 - y
  const min = Math.min(distLeft, distRight, distTop, distBottom)
  if (min === distLeft) {
    return 'left'
  }
  if (min === distRight) {
    return 'right'
  }
  if (min === distTop) {
    return 'top'
  }
  return 'bottom'
}

function DockSplitter({
  orientation,
  onDrag
}: {
  orientation: 'vertical' | 'horizontal'
  onDrag: (deltaPx: number) => void
}) {
  const last = useRef(0)
  return (
    <div
      className={`plugin-splitter plugin-splitter-${orientation}`}
      role="separator"
      aria-orientation={orientation}
      onPointerDown={(e) => {
        if (e.button !== 0) {
          return
        }
        e.preventDefault()
        e.currentTarget.setPointerCapture(e.pointerId)
        last.current = orientation === 'vertical' ? e.clientX : e.clientY
      }}
      onPointerMove={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) {
          return
        }
        const pos = orientation === 'vertical' ? e.clientX : e.clientY
        const delta = pos - last.current
        last.current = pos
        if (delta !== 0) {
          onDrag(delta)
        }
      }}
    />
  )
}

function LayoutTreeView({
  node,
  edge,
  path,
  tabId,
  active,
  plugins,
  settings,
  hostPluginSettings,
  draggingId,
  dropTarget,
  onPluginSettingsPatch,
  onClose,
  onGripPointerDown,
  onSplitRatioChange
}: {
  node: LayoutNode
  edge: DockEdge | 'overlay'
  path: number[]
  tabId: string
  active: boolean
  plugins: PluginListItem[]
  settings: AppSettings
  hostPluginSettings: Record<string, Record<string, unknown>>
  draggingId: string | null
  dropTarget: DropTarget | null
  onPluginSettingsPatch: (pluginId: string, partial: Record<string, unknown>) => void
  onClose: (pluginId: string) => void
  onGripPointerDown: (pluginId: string, event: ReactPointerEvent) => void
  onSplitRatioChange: (edge: DockEdge | 'overlay', path: number[], ratio: number) => void
}): ReactNode {
  const splitRef = useRef<HTMLDivElement>(null)

  if (node.kind === 'leaf') {
    const plugin = plugins.find((p) => p.id === node.pluginId)
    if (!plugin) {
      return null
    }
    const View = getPluginView(plugin.id)
    if (!View) {
      return null
    }
    const pluginSettings = mergePluginSessionSettings(
      plugin,
      settings.pluginSettings[plugin.id],
      hostPluginSettings[plugin.id]
    )
    const props = {
      tabId,
      pluginId: plugin.id,
      settings: pluginSettings,
      onSettingsPatch: (partial: Record<string, unknown>) =>
        onPluginSettingsPatch(plugin.id, partial)
    }
    const title =
      plugin.contributes.views?.[0]?.title || plugin.contributes.toolbar?.label || plugin.name
    const body =
      plugin.id === 'macro-pad' ? (
        <MacroPadView {...props} activeTab={active} />
      ) : (
        <View {...props} />
      )
    const leafDrop =
      dropTarget?.kind === 'leaf' && dropTarget.pluginId === node.pluginId ? dropTarget.zone : null

    return (
      <div
        className="plugin-layout-leaf"
        data-plugin-id={node.pluginId}
        data-drop-zone={leafDrop || undefined}
      >
        {leafDrop ? <div className={`plugin-leaf-drop-preview zone-${leafDrop}`} /> : null}
        <PluginPanelShell
          pluginId={plugin.id}
          title={title}
          dragging={draggingId === plugin.id}
          onClose={onClose}
          onGripPointerDown={onGripPointerDown}
        >
          {body}
        </PluginPanelShell>
      </div>
    )
  }

  const isRow = node.direction === 'row'
  return (
    <div
      ref={splitRef}
      className={`plugin-layout-split plugin-layout-split-${node.direction}`}
    >
      <div className="plugin-layout-split-pane" style={{ flexGrow: node.ratio, flexBasis: 0 }}>
        <LayoutTreeView
          node={node.a}
          edge={edge}
          path={[...path, 0]}
          tabId={tabId}
          active={active}
          plugins={plugins}
          settings={settings}
          hostPluginSettings={hostPluginSettings}
          draggingId={draggingId}
          dropTarget={dropTarget}
          onPluginSettingsPatch={onPluginSettingsPatch}
          onClose={onClose}
          onGripPointerDown={onGripPointerDown}
          onSplitRatioChange={onSplitRatioChange}
        />
      </div>
      <DockSplitter
        orientation={isRow ? 'vertical' : 'horizontal'}
        onDrag={(deltaPx) => {
          const total = isRow
            ? (splitRef.current?.clientWidth ?? 0)
            : (splitRef.current?.clientHeight ?? 0)
          if (total <= 0) {
            return
          }
          onSplitRatioChange(edge, path, clampSplitRatio(node.ratio + deltaPx / total))
        }}
      />
      <div
        className="plugin-layout-split-pane"
        style={{ flexGrow: 1 - node.ratio, flexBasis: 0 }}
      >
        <LayoutTreeView
          node={node.b}
          edge={edge}
          path={[...path, 1]}
          tabId={tabId}
          active={active}
          plugins={plugins}
          settings={settings}
          hostPluginSettings={hostPluginSettings}
          draggingId={draggingId}
          dropTarget={dropTarget}
          onPluginSettingsPatch={onPluginSettingsPatch}
          onClose={onClose}
          onGripPointerDown={onGripPointerDown}
          onSplitRatioChange={onSplitRatioChange}
        />
      </div>
    </div>
  )
}

export default function PluginSessionFrame({
  tabId,
  active,
  plugins,
  activePluginIds,
  layout,
  settings,
  hostPluginSettings,
  onPluginSettingsPatch,
  onLayoutChange,
  onDeactivatePlugin,
  children
}: Props) {
  const frameRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<HTMLDivElement>(null)
  const [draggingId, setDraggingId] = useState<string | null>(null)
  const [dropTarget, setDropTarget] = useState<DropTarget | null>(null)
  const dragPluginRef = useRef<string | null>(null)
  const layoutRef = useRef(layout)
  const onLayoutChangeRef = useRef(onLayoutChange)
  layoutRef.current = layout
  onLayoutChangeRef.current = onLayoutChange

  useEffect(() => {
    let next = pruneLayoutToActive(layoutRef.current, activePluginIds)
    for (const id of activePluginIds) {
      next = ensurePluginInLayout(
        next,
        id,
        plugins.find((p) => p.id === id)
      )
    }
    if (JSON.stringify(layoutRef.current) !== JSON.stringify(next)) {
      onLayoutChangeRef.current(next)
    }
  }, [activePluginIds, plugins])

  const resolveDropTarget = useCallback(
    (clientX: number, clientY: number, movingId: string): DropTarget | null => {
      const frame = frameRef.current
      if (!frame) {
        return null
      }
      const rect = frame.getBoundingClientRect()
      const x = clientX - rect.left
      const y = clientY - rect.top
      const current = layoutRef.current

      // Outer frame bands: dock on the far side of that edge.
      if (x < OUTER_DROP_BAND_PX) {
        return { kind: 'edge', edge: 'left', insert: 'outer' }
      }
      if (x > rect.width - OUTER_DROP_BAND_PX) {
        return { kind: 'edge', edge: 'right', insert: 'outer' }
      }
      if (y < OUTER_DROP_BAND_PX) {
        return { kind: 'edge', edge: 'top', insert: 'outer' }
      }
      if (y > rect.height - OUTER_DROP_BAND_PX) {
        return { kind: 'edge', edge: 'bottom', insert: 'outer' }
      }

      // Inner bands along the terminal: insert between terminal and an existing dock.
      const terminal = terminalRef.current
      if (terminal) {
        const t = terminal.getBoundingClientRect()
        const inTerminalY = clientY >= t.top && clientY <= t.bottom
        const inTerminalX = clientX >= t.left && clientX <= t.right
        if (current.right && inTerminalY && clientX >= t.right - INNER_DROP_BAND_PX && clientX <= t.right + INNER_DROP_BAND_PX) {
          return { kind: 'edge', edge: 'right', insert: 'inner' }
        }
        if (current.left && inTerminalY && clientX >= t.left - INNER_DROP_BAND_PX && clientX <= t.left + INNER_DROP_BAND_PX) {
          return { kind: 'edge', edge: 'left', insert: 'inner' }
        }
        if (current.bottom && inTerminalX && clientY >= t.bottom - INNER_DROP_BAND_PX && clientY <= t.bottom + INNER_DROP_BAND_PX) {
          return { kind: 'edge', edge: 'bottom', insert: 'inner' }
        }
        if (current.top && inTerminalX && clientY >= t.top - INNER_DROP_BAND_PX && clientY <= t.top + INNER_DROP_BAND_PX) {
          return { kind: 'edge', edge: 'top', insert: 'inner' }
        }
      }

      const el = document.elementFromPoint(clientX, clientY)
      const leaf = el?.closest('.plugin-layout-leaf') as HTMLElement | null
      const targetId = leaf?.dataset.pluginId
      if (!leaf || !targetId || targetId === movingId) {
        return null
      }
      const zone = zoneFromPoint(leaf.getBoundingClientRect(), clientX, clientY)
      return { kind: 'leaf', pluginId: targetId, zone }
    },
    []
  )

  const applyDrop = useCallback((movingId: string, target: DropTarget | null) => {
    if (!target) {
      return
    }
    const current = layoutRef.current
    if (target.kind === 'edge') {
      const zone =
        target.insert === 'inner' ? innerInsertZone(target.edge) : outerInsertZone(target.edge)
      onLayoutChangeRef.current(dockPluginOnEdge(current, movingId, target.edge, zone))
      return
    }
    onLayoutChangeRef.current(
      splitPluginLeaf(current, target.pluginId, target.zone, movingId)
    )
  }, [])

  const onGripPointerDown = useCallback(
    (pluginId: string, event: ReactPointerEvent) => {
      dragPluginRef.current = pluginId
      setDraggingId(pluginId)
      const target = event.currentTarget as HTMLElement
      target.setPointerCapture(event.pointerId)

      const onMove = (e: PointerEvent): void => {
        const moving = dragPluginRef.current
        if (!moving) {
          return
        }
        setDropTarget(resolveDropTarget(e.clientX, e.clientY, moving))
      }
      const onUp = (e: PointerEvent): void => {
        target.releasePointerCapture(e.pointerId)
        target.removeEventListener('pointermove', onMove)
        target.removeEventListener('pointerup', onUp)
        target.removeEventListener('pointercancel', onUp)
        const moving = dragPluginRef.current
        const targetDrop = moving
          ? resolveDropTarget(e.clientX, e.clientY, moving)
          : null
        if (moving) {
          applyDrop(moving, targetDrop)
        }
        dragPluginRef.current = null
        setDraggingId(null)
        setDropTarget(null)
      }
      target.addEventListener('pointermove', onMove)
      target.addEventListener('pointerup', onUp)
      target.addEventListener('pointercancel', onUp)
    },
    [applyDrop, resolveDropTarget]
  )

  const onSplitRatioChange = useCallback(
    (edge: DockEdge | 'overlay', path: number[], ratio: number) => {
      onLayoutChangeRef.current(setDockSplitRatio(layoutRef.current, edge, path, ratio))
    },
    []
  )

  const resizeDock = (edge: DockEdge, delta: number): void => {
    const next = { ...layoutRef.current }
    if (edge === 'left') {
      next.leftWidthPx = Math.max(MIN_DOCK_SIZE_PX, next.leftWidthPx + delta)
    } else if (edge === 'right') {
      next.rightWidthPx = Math.max(MIN_DOCK_SIZE_PX, next.rightWidthPx - delta)
    } else if (edge === 'top') {
      next.topHeightPx = Math.max(MIN_DOCK_SIZE_PX, next.topHeightPx + delta)
    } else {
      next.bottomHeightPx = Math.max(MIN_DOCK_SIZE_PX, next.bottomHeightPx - delta)
    }
    onLayoutChangeRef.current(next)
  }

  const renderDock = (edge: DockEdge, node: LayoutNode): ReactNode => {
    const sizeStyle: CSSProperties =
      edge === 'left'
        ? { width: layout.leftWidthPx }
        : edge === 'right'
          ? { width: layout.rightWidthPx }
          : edge === 'top'
            ? { height: layout.topHeightPx }
            : { height: layout.bottomHeightPx }

    return (
      <div
        className={`plugin-dock plugin-dock-${edge}${
          dropTarget?.kind === 'edge' &&
          dropTarget.edge === edge &&
          dropTarget.insert === 'outer'
            ? ' drop-active'
            : ''
        }${
          dropTarget?.kind === 'edge' &&
          dropTarget.edge === edge &&
          dropTarget.insert === 'inner'
            ? ' drop-insert-inner'
            : ''
        }`}
        style={sizeStyle}
      >
        <LayoutTreeView
          node={node}
          edge={edge}
          path={[]}
          tabId={tabId}
          active={active}
          plugins={plugins}
          settings={settings}
          hostPluginSettings={hostPluginSettings}
          draggingId={draggingId}
          dropTarget={dropTarget}
          onPluginSettingsPatch={onPluginSettingsPatch}
          onClose={onDeactivatePlugin}
          onGripPointerDown={onGripPointerDown}
          onSplitRatioChange={onSplitRatioChange}
        />
      </div>
    )
  }

  return (
    <div
      ref={frameRef}
      className={`plugin-session-frame${draggingId ? ' is-rearranging' : ''}`}
    >
      {draggingId ? (
        <>
          <div
            className={`plugin-outer-drop plugin-outer-drop-left${
              dropTarget?.kind === 'edge' &&
              dropTarget.edge === 'left' &&
              dropTarget.insert === 'outer'
                ? ' active'
                : ''
            }`}
          />
          <div
            className={`plugin-outer-drop plugin-outer-drop-right${
              dropTarget?.kind === 'edge' &&
              dropTarget.edge === 'right' &&
              dropTarget.insert === 'outer'
                ? ' active'
                : ''
            }`}
          />
          <div
            className={`plugin-outer-drop plugin-outer-drop-top${
              dropTarget?.kind === 'edge' &&
              dropTarget.edge === 'top' &&
              dropTarget.insert === 'outer'
                ? ' active'
                : ''
            }`}
          />
          <div
            className={`plugin-outer-drop plugin-outer-drop-bottom${
              dropTarget?.kind === 'edge' &&
              dropTarget.edge === 'bottom' &&
              dropTarget.insert === 'outer'
                ? ' active'
                : ''
            }`}
          />
          {layout.bottom ? (
            <div
              className={`plugin-inner-drop plugin-inner-drop-bottom${
                dropTarget?.kind === 'edge' &&
                dropTarget.edge === 'bottom' &&
                dropTarget.insert === 'inner'
                  ? ' active'
                  : ''
              }`}
            />
          ) : null}
        </>
      ) : null}

      {layout.top ? (
        <>
          {renderDock('top', layout.top)}
          <DockSplitter orientation="horizontal" onDrag={(d) => resizeDock('top', d)} />
        </>
      ) : null}

      <div className="plugin-session-mid">
        {layout.left ? (
          <>
            {renderDock('left', layout.left)}
            <DockSplitter orientation="vertical" onDrag={(d) => resizeDock('left', d)} />
          </>
        ) : null}

        <div ref={terminalRef} className="plugin-session-terminal">
          {draggingId && layout.left ? (
            <div
              className={`plugin-inner-drop plugin-inner-drop-left${
                dropTarget?.kind === 'edge' &&
                dropTarget.edge === 'left' &&
                dropTarget.insert === 'inner'
                  ? ' active'
                  : ''
              }`}
            />
          ) : null}
          {draggingId && layout.right ? (
            <div
              className={`plugin-inner-drop plugin-inner-drop-right${
                dropTarget?.kind === 'edge' &&
                dropTarget.edge === 'right' &&
                dropTarget.insert === 'inner'
                  ? ' active'
                  : ''
              }`}
            />
          ) : null}
          {draggingId && layout.top ? (
            <div
              className={`plugin-inner-drop plugin-inner-drop-top${
                dropTarget?.kind === 'edge' &&
                dropTarget.edge === 'top' &&
                dropTarget.insert === 'inner'
                  ? ' active'
                  : ''
              }`}
            />
          ) : null}
          {draggingId && layout.bottom ? (
            <div
              className={`plugin-inner-drop plugin-inner-drop-bottom${
                dropTarget?.kind === 'edge' &&
                dropTarget.edge === 'bottom' &&
                dropTarget.insert === 'inner'
                  ? ' active'
                  : ''
              }`}
            />
          ) : null}
          {children}
        </div>

        {layout.right ? (
          <>
            <DockSplitter orientation="vertical" onDrag={(d) => resizeDock('right', d)} />
            {renderDock('right', layout.right)}
          </>
        ) : null}
      </div>

      {layout.bottom ? (
        <>
          <DockSplitter orientation="horizontal" onDrag={(d) => resizeDock('bottom', d)} />
          {renderDock('bottom', layout.bottom)}
        </>
      ) : null}

      {layout.overlay ? (
        <div className="plugin-dock plugin-dock-overlay">
          <LayoutTreeView
            node={layout.overlay}
            edge="overlay"
            path={[]}
            tabId={tabId}
            active={active}
            plugins={plugins}
            settings={settings}
            hostPluginSettings={hostPluginSettings}
            draggingId={draggingId}
            dropTarget={dropTarget}
            onPluginSettingsPatch={onPluginSettingsPatch}
            onClose={onDeactivatePlugin}
            onGripPointerDown={onGripPointerDown}
            onSplitRatioChange={onSplitRatioChange}
          />
        </div>
      ) : null}
    </div>
  )
}
