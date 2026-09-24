import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
  type ReactNode
} from 'react'
import type { AppSettings } from '@shared/types'
import type { PluginListItem } from '@shared/pluginApi'
import { mergePluginSessionSettings } from '@shared/pluginApi'
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
  pruneLayoutToKeep,
  setDockSplitRatio,
  splitPluginLeaf,
  type DockEdge,
  type LayoutNode,
  type LeafSplitZone,
  type TabPluginLayout
} from '@shared/pluginLayout'
import { getPluginView } from './registry'
import type { PluginViewProps } from './api'
import PluginPanelShell from './PluginPanelShell'

interface Props {
  tabId: string
  /** Saved host profile id of the session; null for unsaved sessions */
  hostId: string | null
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

/** In-progress dock resize: the size it started at plus the size shown now. */
interface DockDrag {
  edge: DockEdge
  /** Dock size when the drag started (px) */
  startSize: number
  /** Dock size currently shown (px) */
  size: number
}

/** True when `target` selects the given edge + insert side. */
function isDropZone(
  target: DropTarget | null,
  edge: DockEdge,
  insert: 'inner' | 'outer'
): boolean {
  return target?.kind === 'edge' && target.edge === edge && target.insert === insert
}

/** Drop-zone overlay div; `active` when the current drop target matches. */
function DropZone({
  className,
  active
}: {
  className: string
  active: boolean
}): ReactElement {
  return <div className={`${className}${active ? ' active' : ''}`} />
}

/** Splitter bar thickness between panes (must match --plugin-splitter-size). */
const PLUGIN_SPLITTER_SIZE_PX = 4

/** Shared, never-replaced path of a dock's layout root (keeps memo props stable). */
const ROOT_SPLIT_PATH: number[] = []

/** Layout field holding the size of each dock. */
const DOCK_SIZE_KEY = {
  left: 'leftWidthPx',
  right: 'rightWidthPx',
  top: 'topHeightPx',
  bottom: 'bottomHeightPx'
} as const satisfies Record<DockEdge, keyof TabPluginLayout>

function dockSizePx(layout: TabPluginLayout, edge: DockEdge): number {
  return layout[DOCK_SIZE_KEY[edge]]
}

function withDockSize(layout: TabPluginLayout, edge: DockEdge, sizePx: number): TabPluginLayout {
  return { ...layout, [DOCK_SIZE_KEY[edge]]: sizePx }
}

/** Largest a dock on `edge` may become so the terminal / mid keeps MIN_DOCK_SIZE_PX. */
function dockMaxSizePx(
  frame: HTMLElement | null,
  cur: TabPluginLayout,
  edge: DockEdge
): number {
  if (!frame) {
    return Number.POSITIVE_INFINITY
  }
  const horizontal = edge === 'left' || edge === 'right'
  const avail = horizontal ? frame.clientWidth : frame.clientHeight
  const other: DockEdge = horizontal
    ? edge === 'left'
      ? 'right'
      : 'left'
    : edge === 'top'
      ? 'bottom'
      : 'top'
  const hasOther = cur[other] !== null
  const reserved =
    (hasOther ? dockSizePx(cur, other) + PLUGIN_SPLITTER_SIZE_PX : 0) +
    PLUGIN_SPLITTER_SIZE_PX +
    MIN_DOCK_SIZE_PX
  return Math.max(MIN_DOCK_SIZE_PX, avail - reserved)
}

/**
 * Clamp every open dock so the terminal / mid area keeps at least
 * MIN_DOCK_SIZE_PX. Returns the same layout reference when nothing changed.
 */
function fitDocksToFrame(
  layout: TabPluginLayout,
  frame: HTMLElement | null
): TabPluginLayout {
  if (!frame) {
    return layout
  }
  let next = layout
  for (const edge of ['left', 'right', 'top', 'bottom'] as DockEdge[]) {
    if (!next[edge]) {
      continue
    }
    const max = dockMaxSizePx(frame, next, edge)
    const clamped = Math.max(MIN_DOCK_SIZE_PX, Math.min(dockSizePx(next, edge), max))
    if (clamped !== dockSizePx(next, edge)) {
      next = withDockSize(next, edge, clamped)
    }
  }
  return next
}

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

/**
 * Splitter bar between panes. Reports the total pointer travel since the drag
 * began, so callers size from a fixed origin: per-event deltas drift because
 * `pointermove` renders coalesce and the size a caller reads back can be stale.
 * `onDragEnd` lets callers hold the live size locally and commit once.
 */
function DockSplitter({
  orientation,
  onDragStart,
  onDrag,
  onDragEnd
}: {
  orientation: 'vertical' | 'horizontal'
  onDragStart: () => void
  onDrag: (totalDeltaPx: number) => void
  onDragEnd: () => void
}): ReactElement {
  const origin = useRef(0)
  const pos = (e: ReactPointerEvent<HTMLDivElement> | PointerEvent): number =>
    orientation === 'vertical' ? e.clientX : e.clientY
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
        origin.current = pos(e)
        onDragStart()
      }}
      onPointerMove={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) {
          return
        }
        onDrag(pos(e) - origin.current)
      }}
      onLostPointerCapture={onDragEnd}
    />
  )
}

interface LayoutTreeViewProps {
  node: LayoutNode
  edge: DockEdge | 'overlay'
  path: number[]
  tabId: string
  hostId: string | null
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
}

function LayoutTreeViewNode({
  node,
  edge,
  path,
  tabId,
  hostId,
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
}: LayoutTreeViewProps): ReactNode {
  const splitRef = useRef<HTMLDivElement>(null)
  /** Live ratio while a splitter in this node is dragged; null when idle. */
  const [dragRatio, setDragRatio] = useState<number | null>(null)
  const dragRef = useRef<{ startRatio: number; ratio: number } | null>(null)
  // Stable child paths: new arrays on every render would defeat the memo above.
  const childPaths = useMemo(() => [[...path, 0], [...path, 1]], [path])

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
      hostId,
      active,
      pluginId: plugin.id,
      settings: pluginSettings,
      onSettingsPatch: (partial: Record<string, unknown>) =>
        onPluginSettingsPatch(plugin.id, partial)
    }
    const title =
      plugin.contributes.views?.[0]?.title || plugin.contributes.toolbar?.label || plugin.name
    const body = <View {...props} />
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
  const ratio = dragRatio ?? node.ratio
  return (
    <div
      ref={splitRef}
      className={`plugin-layout-split plugin-layout-split-${node.direction}`}
    >
      <div className="plugin-layout-split-pane" style={{ flexGrow: ratio, flexBasis: 0 }}>
        <LayoutTreeView
          node={node.a}
          edge={edge}
          path={childPaths[0]}
          tabId={tabId}
          hostId={hostId}
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
        onDragStart={() => {
          dragRef.current = { startRatio: node.ratio, ratio: node.ratio }
        }}
        onDrag={(totalDeltaPx) => {
          const drag = dragRef.current
          const total = isRow
            ? (splitRef.current?.clientWidth ?? 0)
            : (splitRef.current?.clientHeight ?? 0)
          if (!drag || total <= 0) {
            return
          }
          drag.ratio = clampSplitRatio(drag.startRatio + totalDeltaPx / total, total)
          setDragRatio(drag.ratio)
        }}
        onDragEnd={() => {
          const drag = dragRef.current
          dragRef.current = null
          setDragRatio(null)
          if (drag && drag.ratio !== node.ratio) {
            onSplitRatioChange(edge, path, drag.ratio)
          }
        }}
      />
      <div
        className="plugin-layout-split-pane"
        style={{ flexGrow: 1 - ratio, flexBasis: 0 }}
      >
        <LayoutTreeView
          node={node.b}
          edge={edge}
          path={childPaths[1]}
          tabId={tabId}
          hostId={hostId}
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

/**
 * One dock's layout tree. Memoized because resizing a dock re-renders the frame,
 * and the plugin views inside (a long chat list is expensive to reconcile) must
 * not re-render for a resize they do not take part in.
 */
const LayoutTreeView = memo(LayoutTreeViewNode)

export default function PluginSessionFrame({
  tabId,
  hostId,
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
  /**
   * Live dock size while its splitter is dragged: held locally (and mirrored in
   * a ref, since the pointer can outrun a render) so the resize does not push a
   * full app render through on every move.
   */
  const [dockDrag, setDockDrag] = useState<DockDrag | null>(null)
  const dockDragRef = useRef<DockDrag | null>(null)
  const dragPluginRef = useRef<string | null>(null)
  const layoutRef = useRef(layout)
  const onLayoutChangeRef = useRef(onLayoutChange)
  layoutRef.current = layout
  onLayoutChangeRef.current = onLayoutChange

  // Every layout commit first snaps dock sizes to the frame so the terminal /
  // mid area always keeps its MIN_DOCK_SIZE_PX minimum (activating a plugin or
  // drag-docking otherwise inserts a fixed-size dock that can swallow it).
  const commitLayout = useCallback((nextLayout: TabPluginLayout): void => {
    onLayoutChangeRef.current(fitDocksToFrame(nextLayout, frameRef.current))
  }, [])

  const availablePluginIds = useMemo(() => plugins.map((p) => p.id), [plugins])

  // Keep inactive plugins in stored layout (reconnect briefly deactivates them).
  // Only add missing actives; user close removes via onDeactivatePlugin. Panes
  // for plugins no longer registered (removed from the code base) are dropped
  // from the stored layout and never added.
  useEffect(() => {
    let next = pruneLayoutToKeep(layoutRef.current, availablePluginIds)
    for (const id of activePluginIds) {
      const plugin = plugins.find((p) => p.id === id)
      if (plugin) {
        next = ensurePluginInLayout(next, id, plugin)
      }
    }
    if (JSON.stringify(layoutRef.current) !== JSON.stringify(next)) {
      commitLayout(next)
    }
  }, [activePluginIds, plugins, availablePluginIds, commitLayout])

  const displayLayout = useMemo(
    () => pruneLayoutToActive(layout, activePluginIds),
    [layout, activePluginIds]
  )

  // Keep stored dock sizes within the current frame: shrink any dock that would
  // overflow the frame (window resized smaller / restored oversized layout).
  useEffect(() => {
    const frame = frameRef.current
    if (!frame) {
      return
    }
    let raf = 0
    const fit = (): void => {
      cancelAnimationFrame(raf)
      raf = requestAnimationFrame(() => {
        if (frame.clientWidth <= 0 || frame.clientHeight <= 0) {
          return
        }
        const cur = layoutRef.current
        const next = fitDocksToFrame(cur, frame)
        if (next !== cur) {
          commitLayout(next)
        }
      })
    }
    const observer = new ResizeObserver(fit)
    observer.observe(frame)
    return () => {
      observer.disconnect()
      cancelAnimationFrame(raf)
    }
  }, [])

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
      commitLayout(dockPluginOnEdge(current, movingId, target.edge, zone))
      return
    }
    commitLayout(splitPluginLeaf(current, target.pluginId, target.zone, movingId))
  }, [commitLayout])

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
      commitLayout(setDockSplitRatio(layoutRef.current, edge, path, ratio))
    },
    [commitLayout]
  )

  const onDockDragStart = useCallback((edge: DockEdge) => {
    const size = dockSizePx(layoutRef.current, edge)
    const drag: DockDrag = { edge, startSize: size, size }
    dockDragRef.current = drag
    setDockDrag(drag)
  }, [])

  /** Size the dragged dock from its start size, so coalesced moves cannot drift. */
  const onDockDrag = useCallback((edge: DockEdge, totalDeltaPx: number) => {
    const drag = dockDragRef.current
    if (!drag || drag.edge !== edge) {
      return
    }
    const grow = edge === 'left' || edge === 'top' ? totalDeltaPx : -totalDeltaPx
    const max = dockMaxSizePx(frameRef.current, layoutRef.current, edge)
    const next: DockDrag = {
      ...drag,
      size: Math.max(MIN_DOCK_SIZE_PX, Math.min(drag.startSize + grow, max))
    }
    dockDragRef.current = next
    setDockDrag(next)
  }, [])

  const onDockDragEnd = useCallback(() => {
    const drag = dockDragRef.current
    dockDragRef.current = null
    setDockDrag(null)
    if (drag) {
      commitLayout(withDockSize(layoutRef.current, drag.edge, drag.size))
    }
  }, [commitLayout])

  const renderDock = (edge: DockEdge, node: LayoutNode): ReactNode => {
    const size = dockDrag?.edge === edge ? dockDrag.size : dockSizePx(displayLayout, edge)
    const sizeStyle: CSSProperties =
      edge === 'left' || edge === 'right' ? { width: size } : { height: size }

    return (
      <div
        className={`plugin-dock plugin-dock-${edge}${
          isDropZone(dropTarget, edge, 'outer') ? ' drop-active' : ''
        }${isDropZone(dropTarget, edge, 'inner') ? ' drop-insert-inner' : ''}`}
        style={sizeStyle}
      >
        <LayoutTreeView
          node={node}
          edge={edge}
          path={ROOT_SPLIT_PATH}
          tabId={tabId}
          hostId={hostId}
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
          <DropZone
            className="plugin-outer-drop plugin-outer-drop-left"
            active={isDropZone(dropTarget, 'left', 'outer')}
          />
          <DropZone
            className="plugin-outer-drop plugin-outer-drop-right"
            active={isDropZone(dropTarget, 'right', 'outer')}
          />
          <DropZone
            className="plugin-outer-drop plugin-outer-drop-top"
            active={isDropZone(dropTarget, 'top', 'outer')}
          />
          <DropZone
            className="plugin-outer-drop plugin-outer-drop-bottom"
            active={isDropZone(dropTarget, 'bottom', 'outer')}
          />
          {displayLayout.bottom ? (
            <DropZone
              className="plugin-inner-drop plugin-inner-drop-bottom"
              active={isDropZone(dropTarget, 'bottom', 'inner')}
            />
          ) : null}
        </>
      ) : null}

      {displayLayout.top ? (
        <>
          {renderDock('top', displayLayout.top)}
          <DockSplitter
            orientation="horizontal"
            onDragStart={() => onDockDragStart('top')}
            onDrag={(d) => onDockDrag('top', d)}
            onDragEnd={onDockDragEnd}
          />
        </>
      ) : null}

      <div className="plugin-session-mid">
        {displayLayout.left ? (
          <>
            {renderDock('left', displayLayout.left)}
            <DockSplitter
              orientation="vertical"
              onDragStart={() => onDockDragStart('left')}
              onDrag={(d) => onDockDrag('left', d)}
              onDragEnd={onDockDragEnd}
            />
          </>
        ) : null}

        <div ref={terminalRef} className="plugin-session-terminal">
          {draggingId && displayLayout.left ? (
            <DropZone
              className="plugin-inner-drop plugin-inner-drop-left"
              active={isDropZone(dropTarget, 'left', 'inner')}
            />
          ) : null}
          {draggingId && displayLayout.right ? (
            <DropZone
              className="plugin-inner-drop plugin-inner-drop-right"
              active={isDropZone(dropTarget, 'right', 'inner')}
            />
          ) : null}
          {draggingId && displayLayout.top ? (
            <DropZone
              className="plugin-inner-drop plugin-inner-drop-top"
              active={isDropZone(dropTarget, 'top', 'inner')}
            />
          ) : null}
          {draggingId && displayLayout.bottom ? (
            <DropZone
              className="plugin-inner-drop plugin-inner-drop-bottom"
              active={isDropZone(dropTarget, 'bottom', 'inner')}
            />
          ) : null}
          {children}
        </div>

        {displayLayout.right ? (
          <>
            <DockSplitter
              orientation="vertical"
              onDragStart={() => onDockDragStart('right')}
              onDrag={(d) => onDockDrag('right', d)}
              onDragEnd={onDockDragEnd}
            />
            {renderDock('right', displayLayout.right)}
          </>
        ) : null}
      </div>

      {displayLayout.bottom ? (
        <>
          <DockSplitter
            orientation="horizontal"
            onDragStart={() => onDockDragStart('bottom')}
            onDrag={(d) => onDockDrag('bottom', d)}
            onDragEnd={onDockDragEnd}
          />
          {renderDock('bottom', displayLayout.bottom)}
        </>
      ) : null}

      {displayLayout.overlay ? (
        <div className="plugin-dock plugin-dock-overlay">
          <LayoutTreeView
            node={displayLayout.overlay}
            edge="overlay"
            path={ROOT_SPLIT_PATH}
            tabId={tabId}
            hostId={hostId}
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
