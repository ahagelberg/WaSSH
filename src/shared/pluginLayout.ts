import type { PluginListItem, PluginViewPlacement } from './plugins'

/** Side-by-side (horizontal flex) */
export const SPLIT_ROW = 'row'
/** Stacked (vertical flex) */
export const SPLIT_COLUMN = 'column'

export type SplitDirection = typeof SPLIT_ROW | typeof SPLIT_COLUMN

export type DockEdge = 'left' | 'right' | 'top' | 'bottom'

export type DropZone = DockEdge | 'overlay'

/** Which half of a leaf to drop onto */
export type LeafSplitZone = 'left' | 'right' | 'top' | 'bottom'

export type LayoutNode =
  | { kind: 'leaf'; pluginId: string }
  | {
      kind: 'split'
      direction: SplitDirection
      /** Fraction of space for `a` (0–1) */
      ratio: number
      a: LayoutNode
      b: LayoutNode
    }

export interface TabPluginLayout {
  left: LayoutNode | null
  right: LayoutNode | null
  top: LayoutNode | null
  bottom: LayoutNode | null
  overlay: LayoutNode | null
  leftWidthPx: number
  rightWidthPx: number
  topHeightPx: number
  bottomHeightPx: number
}

/** Default dock width (px) */
export const DEFAULT_DOCK_WIDTH_PX = 300

/** Default dock height (px) */
export const DEFAULT_DOCK_HEIGHT_PX = 160

/** Sole minimum for dock edges and split panes (px) */
export const MIN_DOCK_SIZE_PX = 200

/** Default split ratio when creating a split */
export const DEFAULT_SPLIT_RATIO = 0.5

/** Soft ratio bounds when container size is unknown */
const SPLIT_RATIO_FALLBACK_MIN = 0.01
const SPLIT_RATIO_FALLBACK_MAX = 0.99

/** Outer edge hit band while dragging (px) */
export const OUTER_DROP_BAND_PX = 28

/** Band along the terminal edge for inserting between terminal and an existing dock (px) */
export const INNER_DROP_BAND_PX = 36

/** Grow a dock by this much when inserting a sibling column/row beside existing panels (px) */
export const DOCK_INSERT_GROW_PX = 160

/** Zone that inserts toward the terminal (inner side of a dock) */
export function innerInsertZone(edge: DockEdge): LeafSplitZone {
  if (edge === 'left') {
    return 'right'
  }
  if (edge === 'right') {
    return 'left'
  }
  if (edge === 'top') {
    return 'bottom'
  }
  return 'top'
}

/** Zone that inserts on the far outer side of a dock */
export function outerInsertZone(edge: DockEdge): LeafSplitZone {
  return edge
}

export function emptyTabPluginLayout(): TabPluginLayout {
  return {
    left: null,
    right: null,
    top: null,
    bottom: null,
    overlay: null,
    leftWidthPx: DEFAULT_DOCK_WIDTH_PX,
    rightWidthPx: DEFAULT_DOCK_WIDTH_PX,
    topHeightPx: DEFAULT_DOCK_HEIGHT_PX,
    bottomHeightPx: DEFAULT_DOCK_HEIGHT_PX
  }
}

export function clampSplitRatio(ratio: number, totalPx = 0): number {
  if (!Number.isFinite(ratio)) {
    return DEFAULT_SPLIT_RATIO
  }
  if (totalPx > MIN_DOCK_SIZE_PX * 2) {
    const minRatio = MIN_DOCK_SIZE_PX / totalPx
    return Math.min(1 - minRatio, Math.max(minRatio, ratio))
  }
  return Math.min(SPLIT_RATIO_FALLBACK_MAX, Math.max(SPLIT_RATIO_FALLBACK_MIN, ratio))
}

export function collectPluginIds(node: LayoutNode | null): string[] {
  if (!node) {
    return []
  }
  if (node.kind === 'leaf') {
    return [node.pluginId]
  }
  return [...collectPluginIds(node.a), ...collectPluginIds(node.b)]
}

export function layoutPluginIds(layout: TabPluginLayout): string[] {
  return [
    ...collectPluginIds(layout.left),
    ...collectPluginIds(layout.right),
    ...collectPluginIds(layout.top),
    ...collectPluginIds(layout.bottom),
    ...collectPluginIds(layout.overlay)
  ]
}

function removeFromNode(node: LayoutNode, pluginId: string): LayoutNode | null {
  if (node.kind === 'leaf') {
    return node.pluginId === pluginId ? null : node
  }
  const a = removeFromNode(node.a, pluginId)
  const b = removeFromNode(node.b, pluginId)
  if (!a && !b) {
    return null
  }
  if (!a) {
    return b
  }
  if (!b) {
    return a
  }
  return { ...node, a, b }
}

export function removePluginFromLayout(
  layout: TabPluginLayout,
  pluginId: string
): TabPluginLayout {
  return {
    ...layout,
    left: layout.left ? removeFromNode(layout.left, pluginId) : null,
    right: layout.right ? removeFromNode(layout.right, pluginId) : null,
    top: layout.top ? removeFromNode(layout.top, pluginId) : null,
    bottom: layout.bottom ? removeFromNode(layout.bottom, pluginId) : null,
    overlay: layout.overlay ? removeFromNode(layout.overlay, pluginId) : null
  }
}

function edgeKey(edge: DockEdge): keyof Pick<TabPluginLayout, DockEdge> {
  return edge
}

function insertIntoNode(
  node: LayoutNode | null,
  pluginId: string,
  zone: LeafSplitZone
): LayoutNode {
  const leaf: LayoutNode = { kind: 'leaf', pluginId }
  if (!node) {
    return leaf
  }
  if (node.kind === 'leaf') {
    return splitLeafNode(node, leaf, zone)
  }
  // Prefer splitting toward the outer/new side of the whole dock by wrapping root
  return wrapSplit(node, leaf, zone)
}

function splitLeafNode(
  existing: Extract<LayoutNode, { kind: 'leaf' }>,
  incoming: LayoutNode,
  zone: LeafSplitZone
): LayoutNode {
  if (zone === 'left') {
    return {
      kind: 'split',
      direction: SPLIT_ROW,
      ratio: DEFAULT_SPLIT_RATIO,
      a: incoming,
      b: existing
    }
  }
  if (zone === 'right') {
    return {
      kind: 'split',
      direction: SPLIT_ROW,
      ratio: DEFAULT_SPLIT_RATIO,
      a: existing,
      b: incoming
    }
  }
  if (zone === 'top') {
    return {
      kind: 'split',
      direction: SPLIT_COLUMN,
      ratio: DEFAULT_SPLIT_RATIO,
      a: incoming,
      b: existing
    }
  }
  return {
    kind: 'split',
    direction: SPLIT_COLUMN,
    ratio: DEFAULT_SPLIT_RATIO,
    a: existing,
    b: incoming
  }
}

function wrapSplit(existing: LayoutNode, incoming: LayoutNode, zone: LeafSplitZone): LayoutNode {
  if (zone === 'left') {
    return {
      kind: 'split',
      direction: SPLIT_ROW,
      ratio: DEFAULT_SPLIT_RATIO,
      a: incoming,
      b: existing
    }
  }
  if (zone === 'right') {
    return {
      kind: 'split',
      direction: SPLIT_ROW,
      ratio: DEFAULT_SPLIT_RATIO,
      a: existing,
      b: incoming
    }
  }
  if (zone === 'top') {
    return {
      kind: 'split',
      direction: SPLIT_COLUMN,
      ratio: DEFAULT_SPLIT_RATIO,
      a: incoming,
      b: existing
    }
  }
  return {
    kind: 'split',
    direction: SPLIT_COLUMN,
    ratio: DEFAULT_SPLIT_RATIO,
    a: existing,
    b: incoming
  }
}

function mapNode(
  node: LayoutNode,
  targetPluginId: string,
  mapper: (leaf: Extract<LayoutNode, { kind: 'leaf' }>) => LayoutNode
): LayoutNode {
  if (node.kind === 'leaf') {
    return node.pluginId === targetPluginId ? mapper(node) : node
  }
  return {
    ...node,
    a: mapNode(node.a, targetPluginId, mapper),
    b: mapNode(node.b, targetPluginId, mapper)
  }
}

function setNodeRatio(node: LayoutNode, path: number[], ratio: number): LayoutNode {
  if (path.length === 0) {
    if (node.kind !== 'split') {
      return node
    }
    return { ...node, ratio: clampSplitRatio(ratio) }
  }
  if (node.kind !== 'split') {
    return node
  }
  const [head, ...rest] = path
  if (head === 0) {
    return { ...node, a: setNodeRatio(node.a, rest, ratio) }
  }
  return { ...node, b: setNodeRatio(node.b, rest, ratio) }
}

export function setDockSplitRatio(
  layout: TabPluginLayout,
  edge: DockEdge | 'overlay',
  path: number[],
  ratio: number
): TabPluginLayout {
  const key = edge === 'overlay' ? 'overlay' : edgeKey(edge)
  const root = layout[key]
  if (!root) {
    return layout
  }
  return { ...layout, [key]: setNodeRatio(root, path, ratio) }
}

export function dockPluginOnEdge(
  layout: TabPluginLayout,
  pluginId: string,
  edge: DockEdge,
  zone: LeafSplitZone = outerInsertZone(edge)
): TabPluginLayout {
  const cleaned = removePluginFromLayout(layout, pluginId)
  const key = edgeKey(edge)
  const hadContent = cleaned[key] !== null
  const next: TabPluginLayout = {
    ...cleaned,
    [key]: insertIntoNode(cleaned[key], pluginId, zone),
    overlay: cleaned.overlay
  }
  if (!hadContent) {
    return next
  }
  // Inserting beside existing dock content: grow the dock so both can sit sideways/stacked.
  if (edge === 'left' || edge === 'right') {
    const sizeKey = edge === 'left' ? 'leftWidthPx' : 'rightWidthPx'
    next[sizeKey] = cleaned[sizeKey] + DOCK_INSERT_GROW_PX
  } else {
    const sizeKey = edge === 'top' ? 'topHeightPx' : 'bottomHeightPx'
    next[sizeKey] = cleaned[sizeKey] + DOCK_INSERT_GROW_PX
  }
  return next
}

export function splitPluginLeaf(
  layout: TabPluginLayout,
  targetPluginId: string,
  zone: LeafSplitZone,
  pluginId: string
): TabPluginLayout {
  if (targetPluginId === pluginId) {
    return layout
  }
  const cleaned = removePluginFromLayout(layout, pluginId)
  const edges: Array<DockEdge | 'overlay'> = ['left', 'right', 'top', 'bottom', 'overlay']
  for (const edge of edges) {
    const key = edge === 'overlay' ? 'overlay' : edge
    const root = cleaned[key]
    if (!root || !collectPluginIds(root).includes(targetPluginId)) {
      continue
    }
    const next = mapNode(root, targetPluginId, (leaf) =>
      splitLeafNode(leaf, { kind: 'leaf', pluginId }, zone)
    )
    return { ...cleaned, [key]: next }
  }
  return cleaned
}

export function movePluginToOverlay(layout: TabPluginLayout, pluginId: string): TabPluginLayout {
  const cleaned = removePluginFromLayout(layout, pluginId)
  return {
    ...cleaned,
    overlay: insertIntoNode(cleaned.overlay, pluginId, 'bottom')
  }
}

export function placementToEdge(placement: PluginViewPlacement): DockEdge | 'overlay' {
  if (placement === 'overlay') {
    return 'overlay'
  }
  if (placement === 'split-left') {
    return 'left'
  }
  if (placement === 'split-right') {
    return 'right'
  }
  if (placement === 'split-top') {
    return 'top'
  }
  return 'bottom'
}

export function edgeToPlacement(edge: DockEdge | 'overlay'): PluginViewPlacement {
  if (edge === 'overlay') {
    return 'overlay'
  }
  if (edge === 'left') {
    return 'split-left'
  }
  if (edge === 'right') {
    return 'split-right'
  }
  if (edge === 'top') {
    return 'split-top'
  }
  return 'split-bottom'
}

export function findPluginEdge(
  layout: TabPluginLayout,
  pluginId: string
): DockEdge | 'overlay' | null {
  const edges: Array<DockEdge | 'overlay'> = ['left', 'right', 'top', 'bottom', 'overlay']
  for (const edge of edges) {
    const root = edge === 'overlay' ? layout.overlay : layout[edge]
    if (collectPluginIds(root).includes(pluginId)) {
      return edge
    }
  }
  return null
}

export function ensurePluginInLayout(
  layout: TabPluginLayout,
  pluginId: string,
  plugin: PluginListItem | undefined
): TabPluginLayout {
  if (layoutPluginIds(layout).includes(pluginId)) {
    return layout
  }
  const placement = plugin?.contributes.views?.[0]?.placement ?? 'split-right'
  const edge = placementToEdge(placement)
  if (edge === 'overlay') {
    return movePluginToOverlay(layout, pluginId)
  }
  return dockPluginOnEdge(layout, pluginId, edge)
}

export function pruneLayoutToActive(
  layout: TabPluginLayout,
  activePluginIds: string[]
): TabPluginLayout {
  const active = new Set(activePluginIds)
  let next = layout
  for (const id of layoutPluginIds(layout)) {
    if (!active.has(id)) {
      next = removePluginFromLayout(next, id)
    }
  }
  return next
}

function parseNode(raw: unknown): LayoutNode | null {
  if (!raw || typeof raw !== 'object') {
    return null
  }
  const obj = raw as Record<string, unknown>
  if (obj.kind === 'leaf' && typeof obj.pluginId === 'string') {
    return { kind: 'leaf', pluginId: obj.pluginId }
  }
  if (obj.kind === 'split') {
    const a = parseNode(obj.a)
    const b = parseNode(obj.b)
    if (!a || !b) {
      return a || b
    }
    const direction = obj.direction === SPLIT_COLUMN ? SPLIT_COLUMN : SPLIT_ROW
    return {
      kind: 'split',
      direction,
      ratio: clampSplitRatio(Number(obj.ratio)),
      a,
      b
    }
  }
  return null
}

export function normalizeTabPluginLayout(raw: unknown): TabPluginLayout {
  const base = emptyTabPluginLayout()
  if (!raw || typeof raw !== 'object') {
    return base
  }
  const obj = raw as Record<string, unknown>
  return {
    left: parseNode(obj.left),
    right: parseNode(obj.right),
    top: parseNode(obj.top),
    bottom: parseNode(obj.bottom),
    overlay: parseNode(obj.overlay),
    leftWidthPx:
      typeof obj.leftWidthPx === 'number' && obj.leftWidthPx >= MIN_DOCK_SIZE_PX
        ? obj.leftWidthPx
        : base.leftWidthPx,
    rightWidthPx:
      typeof obj.rightWidthPx === 'number' && obj.rightWidthPx >= MIN_DOCK_SIZE_PX
        ? obj.rightWidthPx
        : base.rightWidthPx,
    topHeightPx:
      typeof obj.topHeightPx === 'number' && obj.topHeightPx >= MIN_DOCK_SIZE_PX
        ? obj.topHeightPx
        : base.topHeightPx,
    bottomHeightPx:
      typeof obj.bottomHeightPx === 'number' && obj.bottomHeightPx >= MIN_DOCK_SIZE_PX
        ? obj.bottomHeightPx
        : base.bottomHeightPx
  }
}
