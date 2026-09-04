import { useEffect, useRef, useState, type ReactElement } from 'react'
import {
  MQTT_ANALYSER_BLINK_MS,
  MQTT_ANALYSER_HISTORY_LIMIT,
  type MqttAnalyserErrorKind,
  type MqttAnalyserMessagePayload,
  type MqttAnalyserStatusPayload,
  type MqttAnalyserStatusState
} from '@shared/plugins'
import type { PluginViewProps } from '../registry'

type PublishMode = 'text' | 'json' | 'file'

/** Default fraction of width for the topic tree pane */
const MQTT_SPLIT_DEFAULT_RATIO = 0.42

/** Minimum topic-tree pane fraction when dragging the splitter */
const MQTT_SPLIT_MIN_RATIO = 0.18

/** Maximum topic-tree pane fraction when dragging the splitter */
const MQTT_SPLIT_MAX_RATIO = 0.75

/** QoS used when clearing topics via empty retained publish */
const MQTT_DELETE_QOS = 0 as const

/** Prefix for JSON payload validation errors (also used to mark the textarea) */
const JSON_ERROR_PREFIX = 'Invalid JSON'

function clampSplitRatio(ratio: number): number {
  return Math.min(MQTT_SPLIT_MAX_RATIO, Math.max(MQTT_SPLIT_MIN_RATIO, ratio))
}

interface TopicMessage {
  id: string
  timestamp: number
  qos: 0 | 1 | 2
  retain: boolean
  binary: boolean
  payloadText?: string
  payloadBase64?: string
}

interface TopicNode {
  /** Segment name ('' for empty MQTT level or synthetic root) */
  name: string
  /**
   * Full topic path for this node.
   * Root uses ROOT_PATH so it never collides with an empty leading segment (path '').
   */
  path: string
  children: Map<string, TopicNode>
  /** Recent messages on this exact topic (capped by history limit) */
  messages: TopicMessage[]
  /** Lifetime messages received on this exact topic (not capped by history) */
  receivedCount: number
  /** Self + descendants lifetime message totals */
  messageCount: number
  /** Descendant nodes that have ≥1 message on their exact path */
  subTopicCount: number
  /** Whether this exact path has received at least one message */
  hasOwnMessages: boolean
}

/** Sentinel path for the invisible tree root (not a real MQTT topic) */
const ROOT_PATH = '\0'

function createRoot(): TopicNode {
  return {
    name: '',
    path: ROOT_PATH,
    children: new Map(),
    messages: [],
    receivedCount: 0,
    messageCount: 0,
    subTopicCount: 0,
    hasOwnMessages: false
  }
}

function ensureChild(parent: TopicNode, segment: string, fullPath: string): TopicNode {
  let child = parent.children.get(segment)
  if (!child) {
    child = {
      name: segment,
      path: fullPath,
      children: new Map(),
      messages: [],
      receivedCount: 0,
      messageCount: 0,
      subTopicCount: 0,
      hasOwnMessages: false
    }
    parent.children.set(segment, child)
  } else if (child.path !== fullPath) {
    child.path = fullPath
  }
  return child
}

function topicSegments(topic: string): string[] {
  if (typeof topic !== 'string' || topic.length === 0) {
    return []
  }
  return topic.split('/')
}

/** Join MQTT segments into a full path (`['', 'a']` → `/a`) */
function pathFromSegments(segments: string[]): string {
  return segments.join('/')
}

function recomputeCounts(node: TopicNode): void {
  let subTopics = 0
  let messages = node.receivedCount
  for (const child of Array.from(node.children.values())) {
    recomputeCounts(child)
    messages += child.messageCount
    if (child.hasOwnMessages) {
      subTopics += 1
    }
    subTopics += child.subTopicCount
  }
  node.messageCount = messages
  node.subTopicCount = subTopics
}

function insertMessage(root: TopicNode, msg: TopicMessage, topic: string): TopicNode {
  const segments = topicSegments(topic)
  if (segments.length === 0) {
    return root
  }
  const next = cloneTree(root)
  let node = next
  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i]
    const fullPath = pathFromSegments(segments.slice(0, i + 1))
    node = ensureChild(node, seg, fullPath)
  }
  node.hasOwnMessages = true
  node.receivedCount += 1
  node.messages = [msg, ...node.messages].slice(0, MQTT_ANALYSER_HISTORY_LIMIT)
  recomputeCounts(next)
  return next
}

function cloneTree(node: TopicNode): TopicNode {
  const children = new Map<string, TopicNode>()
  for (const [k, v] of Array.from(node.children.entries())) {
    children.set(k, cloneTree(v))
  }
  return {
    name: node.name,
    path: node.path,
    children,
    messages: node.messages.slice(),
    receivedCount: node.receivedCount,
    messageCount: node.messageCount,
    subTopicCount: node.subTopicCount,
    hasOwnMessages: node.hasOwnMessages
  }
}

function findNode(root: TopicNode, path: string): TopicNode | null {
  const segments = topicSegments(path)
  // path '' is a real empty leading segment under root, not the root itself
  if (path.length === 0) {
    return root.children.get('') ?? null
  }
  let node: TopicNode = root
  for (const seg of segments) {
    const child = node.children.get(seg)
    if (!child) {
      return null
    }
    node = child
  }
  return node
}

function ancestorPaths(topic: string): string[] {
  const segments = topicSegments(topic)
  const paths: string[] = []
  for (let i = 1; i < segments.length; i++) {
    paths.push(pathFromSegments(segments.slice(0, i)))
  }
  return paths
}

/** Label for an empty MQTT topic level in the tree */
const EMPTY_SEGMENT_LABEL = '/'

function segmentLabel(name: string): string {
  return name.length === 0 ? EMPTY_SEGMENT_LABEL : name
}

function displayTopicPath(path: string): string {
  return path.length === 0 ? EMPTY_SEGMENT_LABEL : path
}

/** MQTT publish topic for a tree path (empty leading segment → `/`) */
function mqttTopicFromPath(path: string): string {
  return path.length === 0 ? EMPTY_SEGMENT_LABEL : path
}

function isEmptyPayload(msg: Pick<TopicMessage, 'binary' | 'payloadText' | 'payloadBase64'>): boolean {
  if (msg.binary) {
    return !msg.payloadBase64 || msg.payloadBase64.length === 0
  }
  return (msg.payloadText ?? '').length === 0 && (msg.payloadBase64 ?? '').length === 0
}

/** Topic paths in a subtree, deepest first (children before parent) */
function collectSubtreePaths(node: TopicNode): string[] {
  const paths: string[] = []
  for (const child of Array.from(node.children.values())) {
    paths.push(...collectSubtreePaths(child))
  }
  if (node.path !== ROOT_PATH) {
    paths.push(node.path)
  }
  return paths
}

function childKeyForPath(path: string): string {
  if (path.length === 0) {
    return ''
  }
  const segments = topicSegments(path)
  return segments[segments.length - 1] ?? ''
}

function parentPathOf(path: string): string | null {
  if (path.length === 0) {
    return null
  }
  const segments = topicSegments(path)
  if (segments.length <= 1) {
    return null
  }
  return pathFromSegments(segments.slice(0, -1))
}

/** Clear one topic's messages; drop the node if it has no children */
function clearTopicMessages(root: TopicNode, path: string): TopicNode {
  const next = cloneTree(root)
  const node = findNode(next, path)
  if (!node) {
    return root
  }
  node.messages = []
  node.receivedCount = 0
  node.hasOwnMessages = false
  if (node.children.size === 0) {
    const parentPath = parentPathOf(path)
    const parent = parentPath === null ? next : findNode(next, parentPath)
    if (parent) {
      parent.children.delete(childKeyForPath(path))
      pruneEmptyAncestors(next, parentPath)
    }
  }
  recomputeCounts(next)
  return next
}

function pruneEmptyAncestors(root: TopicNode, startParentPath: string | null): void {
  let path: string | null = startParentPath
  while (path !== null) {
    const node = findNode(root, path)
    if (!node || node.hasOwnMessages || node.children.size > 0) {
      return
    }
    const parentPath = parentPathOf(path)
    const parent = parentPath === null ? root : findNode(root, parentPath)
    if (!parent) {
      return
    }
    parent.children.delete(childKeyForPath(path))
    path = parentPath
  }
}

function formatTime(ts: number): string {
  try {
    return new Date(ts).toLocaleTimeString()
  } catch {
    return String(ts)
  }
}

function formatPayload(msg: TopicMessage): string {
  if (msg.binary) {
    const b64 = msg.payloadBase64 ?? ''
    const preview = b64.length > 120 ? `${b64.slice(0, 120)}…` : b64
    return `(binary, base64) ${preview}`
  }
  const text = msg.payloadText ?? ''
  try {
    const parsed = JSON.parse(text) as unknown
    return JSON.stringify(parsed, null, 2)
  } catch {
    return text
  }
}

function statusLabel(
  state: MqttAnalyserStatusState,
  reason?: string,
  errorKind?: MqttAnalyserErrorKind
): string {
  switch (state) {
    case 'connecting':
      return 'Connecting…'
    case 'connected':
      return 'Connected'
    case 'disconnected':
      return reason ? `Disconnected — ${reason}` : 'Disconnected'
    case 'unavailable':
      return reason || 'SSH only'
    case 'error':
      if (errorKind === 'no_broker') {
        return reason || 'No MQTT broker'
      }
      if (errorKind === 'auth_failed') {
        return reason || 'Authentication failed'
      }
      if (errorKind === 'not_ssh') {
        return reason || 'SSH only'
      }
      return reason || 'Error'
    default:
      return 'Idle'
  }
}

/** Chunk size for base64 encoding large ArrayBuffers without stack overflow */
const BASE64_CHUNK_SIZE = 0x8000

function bufferToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf)
  let binary = ''
  for (let i = 0; i < bytes.length; i += BASE64_CHUNK_SIZE) {
    const slice = bytes.subarray(i, i + BASE64_CHUNK_SIZE)
    binary += String.fromCharCode.apply(null, Array.from(slice) as number[])
  }
  return btoa(binary)
}

function textToBase64(text: string): string {
  const bytes = new TextEncoder().encode(text)
  return bufferToBase64(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength))
}

let messageSeq = 0

function nextMessageId(): string {
  messageSeq += 1
  return `m${messageSeq}`
}

export default function MqttAnalyserView({ tabId, pluginId }: PluginViewProps): ReactElement {
  const [status, setStatus] = useState<MqttAnalyserStatusState>('idle')
  const [statusReason, setStatusReason] = useState<string | undefined>()
  const [errorKind, setErrorKind] = useState<MqttAnalyserErrorKind | undefined>()
  const [root, setRoot] = useState<TopicNode>(() => createRoot())
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set())
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [blinkPaths, setBlinkPaths] = useState<Set<string>>(() => new Set())
  const blinkTimers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map())

  const [publishMode, setPublishMode] = useState<PublishMode>('text')
  const [publishTopic, setPublishTopic] = useState('')
  const [publishText, setPublishText] = useState('')
  const [publishQos, setPublishQos] = useState<0 | 1 | 2>(0)
  const [publishRetain, setPublishRetain] = useState(false)
  const [publishError, setPublishError] = useState<string | null>(null)
  const [fileName, setFileName] = useState<string | null>(null)
  const [fileBase64, setFileBase64] = useState<string | null>(null)
  const [treeRatio, setTreeRatio] = useState(MQTT_SPLIT_DEFAULT_RATIO)
  const splitRef = useRef<HTMLDivElement>(null)
  const splitterLastX = useRef(0)
  const treeContainerRef = useRef<HTMLDivElement>(null)
  const publishErrorRef = useRef<HTMLDivElement>(null)
  const expandedRef = useRef(expanded)
  expandedRef.current = expanded

  useEffect(() => {
    return () => {
      for (const t of Array.from(blinkTimers.current.values())) {
        clearTimeout(t)
      }
      blinkTimers.current.clear()
    }
  }, [])

  const triggerBlink = (paths: string[]): void => {
    setBlinkPaths((prev) => {
      const next = new Set(prev)
      for (const p of paths) {
        next.add(p)
      }
      return next
    })
    for (const p of paths) {
      const existing = blinkTimers.current.get(p)
      if (existing) {
        clearTimeout(existing)
      }
      const timer = setTimeout(() => {
        blinkTimers.current.delete(p)
        setBlinkPaths((prev) => {
          if (!prev.has(p)) {
            return prev
          }
          const next = new Set(prev)
          next.delete(p)
          return next
        })
      }, MQTT_ANALYSER_BLINK_MS)
      blinkTimers.current.set(p, timer)
    }
  }

  useEffect(() => {
    return window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as MqttAnalyserStatusPayload | MqttAnalyserMessagePayload | null
      if (!payload || typeof payload !== 'object' || !('type' in payload)) {
        return
      }
      if (payload.type === 'status') {
        setStatus(payload.state)
        setStatusReason(payload.reason)
        setErrorKind(payload.errorKind)
        return
      }
      if (payload.type === 'message') {
        if (typeof payload.topic !== 'string' || payload.topic.length === 0) {
          return
        }
        const msg: TopicMessage = {
          id: nextMessageId(),
          timestamp: payload.timestamp,
          qos: payload.qos,
          retain: payload.retain,
          binary: payload.binary,
          payloadText: payload.payloadText,
          payloadBase64: payload.payloadBase64
        }
        // Zero-length payload = topic gone (do not add to tree/history)
        if (isEmptyPayload(msg)) {
          setRoot((prev) => clearTopicMessages(prev, payload.topic))
          setSelectedPath((prev) => (prev === payload.topic ? null : prev))
          return
        }
        setRoot((prev) => insertMessage(prev, msg, payload.topic))
        const toBlink = [payload.topic]
        for (const ancestor of ancestorPaths(payload.topic)) {
          if (!expandedRef.current.has(ancestor)) {
            toBlink.push(ancestor)
          }
        }
        triggerBlink(toBlink)
      }
    })
  }, [tabId, pluginId])

  useEffect(() => {
    if (selectedPath !== null) {
      setPublishTopic(displayTopicPath(selectedPath) === EMPTY_SEGMENT_LABEL ? '/' : selectedPath)
    }
  }, [selectedPath])

  useEffect(() => {
    if (publishError === null || !publishErrorRef.current) {
      return
    }
    publishErrorRef.current.scrollIntoView({ block: 'nearest' })
  }, [publishError])

  const selectedNode = selectedPath !== null ? findNode(root, selectedPath) : null

  const toggleExpand = (path: string): void => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(path)) {
        next.delete(path)
      } else {
        next.add(path)
      }
      return next
    })
  }

  const reconnect = (): void => {
    setRoot(createRoot())
    setSelectedPath(null)
    void window.wassh.sendPluginMessage(tabId, pluginId, { type: 'reconnect' })
  }

  const handlePublish = async (): Promise<void> => {
    setPublishError(null)
    const topic = publishTopic.trim()
    if (!topic) {
      setPublishError('Topic is required')
      return
    }
    if (status !== 'connected') {
      setPublishError('Not connected')
      return
    }

    let payloadBase64 = ''
    if (publishMode === 'file') {
      if (!fileBase64) {
        setPublishError('Choose a file')
        return
      }
      payloadBase64 = fileBase64
    } else {
      let text = publishText
      if (publishMode === 'json') {
        try {
          text = JSON.stringify(JSON.parse(publishText) as unknown)
        } catch (err) {
          const detail = err instanceof Error ? err.message : 'parse failed'
          setPublishError(`${JSON_ERROR_PREFIX} — ${detail}`)
          return
        }
      }
      try {
        payloadBase64 = textToBase64(text)
      } catch (err) {
        setPublishError(err instanceof Error ? err.message : 'Failed to encode payload')
        return
      }
    }

    try {
      const result = await window.wassh.sendPluginMessage(tabId, pluginId, {
        type: 'publish',
        topic,
        payloadBase64,
        qos: publishQos,
        retain: publishRetain
      })
      if (typeof result === 'string' && result.length > 0) {
        setPublishError(result)
      }
    } catch (err) {
      setPublishError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleDelete = async (): Promise<void> => {
    if (selectedPath === null || status !== 'connected') {
      return
    }
    const node = findNode(root, selectedPath)
    if (!node) {
      return
    }
    const paths = collectSubtreePaths(node)
    for (const path of paths) {
      await window.wassh.sendPluginMessage(tabId, pluginId, {
        type: 'publish',
        topic: mqttTopicFromPath(path),
        payloadBase64: '',
        qos: MQTT_DELETE_QOS,
        retain: true
      })
      setRoot((prev) => clearTopicMessages(prev, path))
    }
    setSelectedPath(null)
  }

  const onFileChange = async (file: File | null): Promise<void> => {
    if (!file) {
      setFileName(null)
      setFileBase64(null)
      return
    }
    const buf = await file.arrayBuffer()
    setFileName(file.name)
    setFileBase64(bufferToBase64(buf))
  }

  const sortedChildren = (node: TopicNode): TopicNode[] =>
    Array.from(node.children.values()).sort((a, b) => a.name.localeCompare(b.name))

  // Flat, depth-first list of rows currently visible in the tree (expanded
  // children only) plus each node's parent, for keyboard navigation.
  const visibleNodes: TopicNode[] = []
  const parentOfPath = new Map<string, string>()
  const collectVisible = (node: TopicNode): void => {
    for (const child of sortedChildren(node)) {
      visibleNodes.push(child)
      parentOfPath.set(child.path, node.path)
      if (expanded.has(child.path)) {
        collectVisible(child)
      }
    }
  }
  collectVisible(root)

  const selectedVisibleIndex = selectedPath
    ? visibleNodes.findIndex((n) => n.path === selectedPath)
    : -1

  const expandPath = (path: string): void => {
    setExpanded((prev) => {
      const next = new Set(prev)
      next.add(path)
      return next
    })
  }

  const collapsePath = (path: string): void => {
    setExpanded((prev) => {
      const next = new Set(prev)
      next.delete(path)
      return next
    })
  }

  useEffect(() => {
    if (selectedPath === null || !treeContainerRef.current) {
      return
    }
    for (const el of Array.from(
      treeContainerRef.current.querySelectorAll<HTMLElement>('[data-mqtt-path]')
    )) {
      if (el.dataset.mqttPath === selectedPath) {
        el.focus({ preventScroll: true })
        el.scrollIntoView({ block: 'nearest' })
        break
      }
    }
  }, [selectedPath])

  const renderTreeNode = (node: TopicNode): ReactElement[] => {
    const elements: ReactElement[] = []
    for (const child of sortedChildren(node)) {
      const hasChildren = child.children.size > 0
      const isExpanded = expanded.has(child.path)
      const isSelected = selectedPath === child.path
      const isBlinking = blinkPaths.has(child.path)
      const showCollapsedMeta = hasChildren && !isExpanded

      elements.push(
        <div key={child.path} className="mqtt-tree-row-wrap">
          <button
            type="button"
            className={[
              'mqtt-tree-row',
              isSelected ? 'selected' : '',
              isBlinking ? 'blink' : ''
            ]
              .filter(Boolean)
              .join(' ')}
            data-mqtt-path={child.path}
            onClick={() => setSelectedPath(child.path)}
            onDoubleClick={() => {
              if (hasChildren) {
                toggleExpand(child.path)
              }
            }}
          >
            {hasChildren ? (
              <span
                className="mqtt-tree-twist"
                role="presentation"
                onClick={(e) => {
                  e.stopPropagation()
                  toggleExpand(child.path)
                }}
              >
                {isExpanded ? '▾' : '▸'}
              </span>
            ) : (
              <span className="mqtt-tree-twist mqtt-tree-twist-empty" />
            )}
            <span className="mqtt-tree-name">{segmentLabel(child.name)}</span>
            {showCollapsedMeta ? (
              <span className="mqtt-tree-meta">
                {child.subTopicCount} topic{child.subTopicCount === 1 ? '' : 's'},{' '}
                {child.messageCount} msg{child.messageCount === 1 ? '' : 's'}
              </span>
            ) : child.hasOwnMessages ? (
              <span className="mqtt-tree-meta">{child.receivedCount}</span>
            ) : null}
          </button>
          {hasChildren && isExpanded ? (
            <div className="mqtt-tree-children">{renderTreeNode(child)}</div>
          ) : null}
        </div>
      )
    }
    return elements
  }

  return (
    <div
      className="plugin-panel mqtt-analyser"
      tabIndex={0}
      onMouseDown={(e) => {
        const target = e.target as HTMLElement
        if (target.closest('input, textarea, select')) {
          return
        }
        e.currentTarget.focus({ preventScroll: true })
      }}
      onKeyDown={(e) => {
        const target = e.target as HTMLElement
        if (target.closest('input, textarea, select')) {
          return
        }
        if (e.key === 'Delete') {
          if (selectedPath === null || status !== 'connected') {
            return
          }
          e.preventDefault()
          void handleDelete()
          return
        }
        if (!e.key.startsWith('Arrow')) {
          return
        }
        e.preventDefault()
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          const delta = e.key === 'ArrowDown' ? 1 : -1
          const next = visibleNodes[selectedVisibleIndex + delta]
          if (next) {
            setSelectedPath(next.path)
          } else if (selectedVisibleIndex < 0 && visibleNodes.length > 0) {
            setSelectedPath(visibleNodes[0].path)
          }
          return
        }
        const node = selectedPath !== null ? findNode(root, selectedPath) : null
        if (e.key === 'ArrowRight') {
          if (node && node.children.size > 0) {
            if (expanded.has(node.path)) {
              const firstChild = sortedChildren(node)[0]
              if (firstChild) {
                setSelectedPath(firstChild.path)
              }
            } else {
              expandPath(node.path)
            }
          }
          return
        }
        // ArrowLeft
        if (node && node.children.size > 0 && expanded.has(node.path)) {
          collapsePath(node.path)
          return
        }
        if (selectedPath !== null) {
          const parent = parentOfPath.get(selectedPath)
          if (parent !== undefined && parent !== ROOT_PATH) {
            setSelectedPath(parent)
          }
        }
      }}
    >
      <div className="mqtt-status-bar">
        <span
          className={[
            'mqtt-status',
            status === 'connected' ? 'ok' : '',
            status === 'error' || status === 'unavailable' ? 'bad' : ''
          ]
            .filter(Boolean)
            .join(' ')}
        >
          {statusLabel(status, statusReason, errorKind)}
        </span>
        <button
          type="button"
          className="mqtt-reconnect-btn"
          onClick={reconnect}
          disabled={status === 'unavailable' && errorKind === 'not_ssh'}
        >
          Reconnect
        </button>
      </div>

      <div className="mqtt-split" ref={splitRef}>
        <div
          className="mqtt-tree-pane"
          style={{ flexGrow: treeRatio, flexBasis: 0 }}
        >
          <div className="mqtt-pane-label">Topics</div>
          <div className="mqtt-tree" ref={treeContainerRef}>
            {root.children.size === 0 ? (
              <div className="mqtt-empty">No messages yet</div>
            ) : (
              renderTreeNode(root)
            )}
          </div>
        </div>

        <div
          className="plugin-splitter plugin-splitter-vertical mqtt-splitter"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize topic and detail panes"
          onPointerDown={(e) => {
            if (e.button !== 0) {
              return
            }
            e.preventDefault()
            e.currentTarget.setPointerCapture(e.pointerId)
            splitterLastX.current = e.clientX
          }}
          onPointerMove={(e) => {
            if (!e.currentTarget.hasPointerCapture(e.pointerId)) {
              return
            }
            const total = splitRef.current?.clientWidth ?? 0
            if (total <= 0) {
              return
            }
            const delta = e.clientX - splitterLastX.current
            splitterLastX.current = e.clientX
            if (delta === 0) {
              return
            }
            setTreeRatio((prev) => clampSplitRatio(prev + delta / total))
          }}
        />

        <div
          className="mqtt-detail-pane"
          style={{ flexGrow: 1 - treeRatio, flexBasis: 0 }}
        >
          <div className="mqtt-pane-label">Details</div>
          {selectedPath === null || !selectedNode ? (
            <div className="mqtt-empty">Select a topic</div>
          ) : (
            <>
              <div className="mqtt-detail-meta">
                <div className="mqtt-detail-topic-row">
                  <div className="mqtt-detail-topic" title={displayTopicPath(selectedPath)}>
                    {displayTopicPath(selectedPath)}
                  </div>
                  <button
                    type="button"
                    className="danger mqtt-delete-btn"
                    onClick={() => void handleDelete()}
                    disabled={status !== 'connected'}
                    title="Remove topic by publishing an empty retained message"
                  >
                    Delete
                  </button>
                </div>
                <div className="mqtt-detail-facts">
                  <span>{selectedNode.messages.length} in history</span>
                  <span>{selectedNode.subTopicCount} sub-topics</span>
                  <span>{selectedNode.messageCount} msgs total</span>
                </div>
              </div>

              <div className="mqtt-history">
                {selectedNode.messages.length === 0 ? (
                  <div className="mqtt-empty">
                    {selectedNode.children.size > 0
                      ? 'Branch node — expand and select a leaf topic with messages'
                      : 'No messages on this topic'}
                  </div>
                ) : (
                  selectedNode.messages.map((msg) => (
                    <div key={msg.id} className="mqtt-history-item">
                      <div className="mqtt-history-head">
                        <span>{formatTime(msg.timestamp)}</span>
                        <span>QoS {msg.qos}</span>
                        {msg.retain ? <span className="mqtt-retain">retain</span> : null}
                        {msg.binary ? <span>binary</span> : null}
                      </div>
                      <pre className="mqtt-history-payload">{formatPayload(msg)}</pre>
                    </div>
                  ))
                )}
              </div>
            </>
          )}

          <div className="mqtt-publish">
            <div className="mqtt-pane-label">Publish</div>
            <label className="mqtt-field">
              <span>Topic</span>
              <input
                type="text"
                value={publishTopic}
                onChange={(e) => setPublishTopic(e.target.value)}
              />
            </label>
            <div className="mqtt-publish-modes">
              {(['text', 'json', 'file'] as PublishMode[]).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  className={publishMode === mode ? 'active' : ''}
                  onClick={() => setPublishMode(mode)}
                >
                  {mode === 'file' ? 'Raw file' : mode === 'json' ? 'JSON' : 'Text'}
                </button>
              ))}
            </div>
            {publishMode === 'file' ? (
              <label className="mqtt-field">
                <span>File</span>
                <input
                  type="file"
                  onChange={(e) => {
                    void onFileChange(e.target.files?.[0] ?? null)
                  }}
                />
                {fileName ? <span className="mqtt-file-name">{fileName}</span> : null}
              </label>
            ) : (
              <label className="mqtt-field">
                <span>Payload</span>
                <textarea
                  value={publishText}
                  onChange={(e) => setPublishText(e.target.value)}
                  rows={4}
                  spellCheck={false}
                />
              </label>
            )}
            <div className="mqtt-publish-options">
              <label>
                QoS
                <select
                  value={publishQos}
                  onChange={(e) => setPublishQos(Number(e.target.value) as 0 | 1 | 2)}
                >
                  <option value={0}>0</option>
                  <option value={1}>1</option>
                  <option value={2}>2</option>
                </select>
              </label>
              <label className="mqtt-retain-check">
                <input
                  type="checkbox"
                  checked={publishRetain}
                  onChange={(e) => setPublishRetain(e.target.checked)}
                />
                Retain
              </label>
              <button type="button" onClick={() => void handlePublish()}>
                Publish
              </button>
            </div>
            {publishError ? <div className="mqtt-publish-error">{publishError}</div> : null}
          </div>
        </div>
      </div>
    </div>
  )
}
