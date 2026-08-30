import { useEffect, useRef, useState, type ReactElement } from 'react'
import {
  MQTT_EXPLORER_BLINK_MS,
  MQTT_EXPLORER_HISTORY_LIMIT,
  type MqttExplorerErrorKind,
  type MqttExplorerMessagePayload,
  type MqttExplorerStatusPayload,
  type MqttExplorerStatusState
} from '@shared/plugins'
import type { PluginViewProps } from '../registry'

type PublishMode = 'text' | 'json' | 'file'

/** Default fraction of width for the topic tree pane */
const MQTT_SPLIT_DEFAULT_RATIO = 0.42

/** Minimum topic-tree pane fraction when dragging the splitter */
const MQTT_SPLIT_MIN_RATIO = 0.18

/** Maximum topic-tree pane fraction when dragging the splitter */
const MQTT_SPLIT_MAX_RATIO = 0.75

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
  /** Messages received on this exact topic */
  messages: TopicMessage[]
  /** Self + descendants */
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
  let messages = node.messages.length
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
  node.messages = [msg, ...node.messages].slice(0, MQTT_EXPLORER_HISTORY_LIMIT)
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
  state: MqttExplorerStatusState,
  reason?: string,
  errorKind?: MqttExplorerErrorKind
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

export default function MqttExplorerView({ tabId, pluginId }: PluginViewProps): ReactElement {
  const [status, setStatus] = useState<MqttExplorerStatusState>('idle')
  const [statusReason, setStatusReason] = useState<string | undefined>()
  const [errorKind, setErrorKind] = useState<MqttExplorerErrorKind | undefined>()
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
      }, MQTT_EXPLORER_BLINK_MS)
      blinkTimers.current.set(p, timer)
    }
  }

  useEffect(() => {
    return window.wassh.onPluginMessage((ev) => {
      if (ev.tabId !== tabId || ev.pluginId !== pluginId) {
        return
      }
      const payload = ev.payload as MqttExplorerStatusPayload | MqttExplorerMessagePayload | null
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
    } else if (publishMode === 'json') {
      try {
        JSON.parse(publishText)
      } catch {
        setPublishError('Invalid JSON')
        return
      }
      payloadBase64 = textToBase64(publishText)
    } else {
      payloadBase64 = textToBase64(publishText)
    }

    await window.wassh.sendPluginMessage(tabId, pluginId, {
      type: 'publish',
      topic,
      payloadBase64,
      qos: publishQos,
      retain: publishRetain
    })
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
            onClick={() => setSelectedPath(child.path)}
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
              <span className="mqtt-tree-meta">{child.messages.length}</span>
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
    <div className="plugin-panel mqtt-explorer">
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
          <div className="mqtt-tree">
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
                <div className="mqtt-detail-topic" title={displayTopicPath(selectedPath)}>
                  {displayTopicPath(selectedPath)}
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
            </>
          )}
        </div>
      </div>
    </div>
  )
}
