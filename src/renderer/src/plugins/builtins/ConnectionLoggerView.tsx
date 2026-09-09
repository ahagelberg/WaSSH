import { useEffect, useMemo, useRef, useState, type ReactElement } from 'react'
import type {
  ConnectionLogEntry,
  ConnectionLogEventKind,
  ConnectionLoggerData,
  ConnectionLoggerGraphMode,
  ConnectionLoggerTimeWindow
} from '@shared/plugins'
import type { SessionStatus } from '@shared/types'
import { PLUGIN_ID_CONNECTION_LOGGER } from '@shared/plugins'
import type { PluginViewProps } from '../registry'

function scopeIdFor(hostId: string | null, tabId: string): string {
  return hostId || `tab:${tabId}`
}

function formatDuration(ms: number | undefined): string {
  if (ms === undefined || ms < 0) return '—'
  const seconds = Math.floor(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  const remainingSec = seconds % 60
  if (minutes < 60) return `${minutes}m ${remainingSec}s`
  const hours = Math.floor(minutes / 60)
  const remainingMin = minutes % 60
  if (hours < 24) return `${hours}h ${remainingMin}m`
  const days = Math.floor(hours / 24)
  const remainingHours = hours % 24
  return `${days}d ${remainingHours}h`
}

function formatTimestamp(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => n.toString().padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

function formatShortTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => n.toString().padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`
}

interface TimeSpan {
  id: string
  start: number
  end: number
  kind: ConnectionLogEventKind
  message?: string
}

function buildTimeSpans(
  events: ConnectionLogEntry[],
  windowStart: number,
  windowEnd: number,
  currentStatus: SessionStatus
): TimeSpan[] {
  if (events.length === 0) {
    const fallbackKind: ConnectionLogEventKind =
      currentStatus === 'connected' ? 'connected' : 'disconnected'
    return [
      {
        id: 'current-only',
        start: windowStart,
        end: windowEnd,
        kind: fallbackKind
      }
    ]
  }

  // Sort events chronologically
  const sorted = [...events].sort((a, b) => a.timestamp - b.timestamp)
  const spans: TimeSpan[] = []

  for (let i = 0; i < sorted.length; i++) {
    const current = sorted[i]
    const next = sorted[i + 1]
    const spanStart = current.timestamp
    const spanEnd = next ? next.timestamp : windowEnd

    // Clip to window
    const clippedStart = Math.max(windowStart, spanStart)
    const clippedEnd = Math.min(windowEnd, spanEnd)

    if (clippedEnd > clippedStart) {
      spans.push({
        id: current.id,
        start: clippedStart,
        end: clippedEnd,
        kind: current.kind,
        message: current.message
      })
    }
  }

  // If the earliest event is after windowStart, add a preceding span based on first event
  if (sorted[0] && sorted[0].timestamp > windowStart) {
    const firstKind = sorted[0].kind
    // If first event was 'connected', preceding was disconnected
    const precedingKind: ConnectionLogEventKind =
      firstKind === 'connected' ? 'disconnected' : 'connected'
    spans.unshift({
      id: 'pre-window',
      start: windowStart,
      end: Math.min(windowEnd, sorted[0].timestamp),
      kind: precedingKind
    })
  }

  return spans
}

export default function ConnectionLoggerView({ tabId, hostId }: PluginViewProps) {
  const scope = scopeIdFor(hostId, tabId)
  const [events, setEvents] = useState<ConnectionLogEntry[]>([])
  const [graphMode, setGraphMode] = useState<ConnectionLoggerGraphMode>('timeline')
  const [timeWindow, setTimeWindow] = useState<ConnectionLoggerTimeWindow>('24h')
  const [filterKind, setFilterKind] = useState<string>('all')
  const [searchQuery, setSearchQuery] = useState<string>('')
  const [showClearConfirm, setShowClearConfirm] = useState(false)
  const [now, setNow] = useState(Date.now())
  const [hoveredSpan, setHoveredSpan] = useState<TimeSpan | null>(null)
  const [hoveredCell, setHoveredCell] = useState<{ label: string; uptimePct: number; connectedMs: number; totalMs: number; disconnects: number } | null>(null)
  const [currentStatus, setCurrentStatus] = useState<SessionStatus>('connected')

  // Keep 'now' updated every 5 seconds for live graphs and tickers
  useEffect(() => {
    const interval = setInterval(() => setNow(Date.now()), 5000)
    return () => clearInterval(interval)
  }, [])

  // Load initial data
  useEffect(() => {
    let cancelled = false
    void (async () => {
      const raw = await window.wassh.getPluginData(PLUGIN_ID_CONNECTION_LOGGER, scope)
      if (cancelled) return
      if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
        const data = raw as ConnectionLoggerData
        if (Array.isArray(data.events)) {
          setEvents(data.events)
        }
      }
      void window.wassh.sendPluginMessage(tabId, PLUGIN_ID_CONNECTION_LOGGER, {
        type: 'sync',
        scopeId: scope
      })
    })()

    return () => {
      cancelled = true
    }
  }, [tabId, scope])

  // Listen to plugin messages & session status
  useEffect(() => {
    const offMsg = window.wassh.onPluginMessage((ev) => {
      if (ev.pluginId !== PLUGIN_ID_CONNECTION_LOGGER) return
      const payload = ev.payload as Record<string, unknown>
      if (!payload) return

      if (payload.type === 'state') {
        const evList = payload.events as ConnectionLogEntry[] | undefined
        if (Array.isArray(evList)) {
          setEvents(evList)
        }
      } else if (payload.type === 'event') {
        const newEntry = payload.entry as ConnectionLogEntry | undefined
        if (newEntry) {
          setEvents((prev) => {
            if (prev.some((e) => e.id === newEntry.id)) return prev
            return [...prev, newEntry]
          })
        }
        if (typeof payload.currentStatus === 'string') {
          setCurrentStatus(payload.currentStatus as SessionStatus)
        }
      }
    })

    const offStatus = window.wassh.onSessionStatus((ev) => {
      if (ev.tabId === tabId) {
        setCurrentStatus(ev.status)
      }
    })

    return () => {
      offMsg()
      offStatus()
    }
  }, [tabId])

  // Compute time window boundaries
  const windowRange = useMemo(() => {
    const end = now
    let durationMs = 24 * 3600 * 1000
    if (timeWindow === '1h') durationMs = 3600 * 1000
    else if (timeWindow === '6h') durationMs = 6 * 3600 * 1000
    else if (timeWindow === '24h') durationMs = 24 * 3600 * 1000
    else if (timeWindow === '7d') durationMs = 7 * 86400 * 1000
    else if (timeWindow === 'all') {
      const earliest = events.reduce((min, e) => Math.min(min, e.timestamp), now)
      durationMs = Math.max(3600 * 1000, now - earliest + 60000)
    }
    const start = end - durationMs
    return { start, end, durationMs }
  }, [timeWindow, events, now])

  // Build continuous time spans for the selected window
  const spans = useMemo(() => {
    return buildTimeSpans(events, windowRange.start, windowRange.end, currentStatus)
  }, [events, windowRange, currentStatus])

  // Compute summary statistics
  const stats = useMemo(() => {
    let connectedMs = 0
    let disconnectedMs = 0
    let disconnectCount = 0
    const totalMs = Math.max(1, windowRange.end - windowRange.start)

    for (const span of spans) {
      const spanDur = Math.max(0, span.end - span.start)
      if (span.kind === 'connected') {
        connectedMs += spanDur
      } else {
        disconnectedMs += spanDur
        if (span.kind === 'disconnected' || span.kind === 'failed') {
          disconnectCount++
        }
      }
    }

    const uptimePct = Math.min(100, Math.max(0, (connectedMs / totalMs) * 100))

    // Average connected session length
    const connectedSpans = spans.filter((s) => s.kind === 'connected')
    const avgConnectedMs =
      connectedSpans.length > 0
        ? connectedSpans.reduce((sum, s) => sum + (s.end - s.start), 0) / connectedSpans.length
        : 0

    // Latest state duration
    const latestEvent = events[events.length - 1]
    const currentDurationMs = latestEvent ? Math.max(0, now - latestEvent.timestamp) : 0

    return {
      uptimePct,
      connectedMs,
      disconnectedMs,
      disconnectCount,
      avgConnectedMs,
      currentDurationMs,
      totalEvents: events.length
    }
  }, [spans, windowRange, events, now])

  // Filtered log events for table
  const filteredEvents = useMemo(() => {
    return [...events]
      .reverse()
      .filter((e) => {
        if (filterKind !== 'all' && e.kind !== filterKind) return false
        if (searchQuery.trim()) {
          const q = searchQuery.toLowerCase()
          const matchMsg = e.message?.toLowerCase().includes(q)
          const matchKind = e.kind.toLowerCase().includes(q)
          if (!matchMsg && !matchKind) return false
        }
        return true
      })
  }, [events, filterKind, searchQuery])

  // Export handlers
  const exportCsv = () => {
    const headers = ['Timestamp', 'Date/Time', 'Event', 'Duration(s)', 'Message']
    const rows = events.map((e) => [
      e.timestamp,
      formatTimestamp(e.timestamp),
      e.kind,
      e.durationMs ? (e.durationMs / 1000).toFixed(1) : '',
      `"${(e.message || '').replace(/"/g, '""')}"`
    ])
    const csvContent = [headers.join(','), ...rows.map((r) => r.join(','))].join('\n')
    const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `connection-log-${scope}.csv`
    a.click()
    URL.revokeObjectURL(url)
  }

  const exportJson = () => {
    const jsonContent = JSON.stringify({ scope, exportedAt: new Date().toISOString(), events }, null, 2)
    const blob = new Blob([jsonContent], { type: 'application/json;charset=utf-8;' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `connection-log-${scope}.json`
    a.click()
    URL.revokeObjectURL(url)
  }

  const clearLogs = () => {
    void window.wassh.sendPluginMessage(tabId, PLUGIN_ID_CONNECTION_LOGGER, {
      type: 'clearLogs',
      scopeId: scope
    })
    setEvents([])
    setShowClearConfirm(false)
  }

  // Render Timeline Bar Graph
  const renderTimelineBar = (): ReactElement => {
    const totalMs = windowRange.durationMs
    return (
      <div className="conn-log-graph-box">
        <div className="conn-log-timeline-bar" role="img" aria-label="Connection timeline">
          {spans.map((span, idx) => {
            const leftPct = ((span.start - windowRange.start) / totalMs) * 100
            const widthPct = Math.max(0.4, ((span.end - span.start) / totalMs) * 100)
            return (
              <div
                key={`${span.id}-${idx}`}
                className={`conn-log-timeline-segment segment-${span.kind}`}
                style={{ left: `${leftPct}%`, width: `${widthPct}%` }}
                onMouseEnter={() => setHoveredSpan(span)}
                onMouseLeave={() => setHoveredSpan(null)}
              />
            )
          })}
        </div>
        <div className="conn-log-time-axis">
          <span>{formatTimestamp(windowRange.start)}</span>
          <span>{formatShortTime(windowRange.start + totalMs * 0.25)}</span>
          <span>{formatShortTime(windowRange.start + totalMs * 0.5)}</span>
          <span>{formatShortTime(windowRange.start + totalMs * 0.75)}</span>
          <span>Now ({formatShortTime(windowRange.end)})</span>
        </div>
        {hoveredSpan ? (
          <div className="conn-log-tooltip">
            <strong>{hoveredSpan.kind.toUpperCase()}</strong>
            <span>
              {formatShortTime(hoveredSpan.start)} → {formatShortTime(hoveredSpan.end)} (
              {formatDuration(hoveredSpan.end - hoveredSpan.start)})
            </span>
            {hoveredSpan.message ? <em>{hoveredSpan.message}</em> : null}
          </div>
        ) : null}
      </div>
    )
  }

  // Render Step Line / Area Chart
  const renderStepChart = (): ReactElement => {
    const svgWidth = 800
    const svgHeight = 100
    const totalMs = windowRange.durationMs
    const padding = 10
    const chartHeight = svgHeight - padding * 2

    // Build SVG path points
    const points: Array<{ x: number; y: number }> = []

    for (const span of spans) {
      const x1 = ((span.start - windowRange.start) / totalMs) * svgWidth
      const x2 = ((span.end - windowRange.start) / totalMs) * svgWidth
      const isUp = span.kind === 'connected'
      const y = isUp ? padding : padding + chartHeight

      points.push({ x: x1, y })
      points.push({ x: x2, y })
    }

    if (points.length === 0) {
      points.push({ x: 0, y: padding + chartHeight })
      points.push({ x: svgWidth, y: padding + chartHeight })
    }

    let linePath = `M ${points[0].x.toFixed(1)} ${points[0].y.toFixed(1)}`
    for (let i = 1; i < points.length; i++) {
      linePath += ` L ${points[i].x.toFixed(1)} ${points[i].y.toFixed(1)}`
    }

    const areaPath = `${linePath} L ${svgWidth} ${svgHeight - padding} L 0 ${svgHeight - padding} Z`

    return (
      <div className="conn-log-graph-box">
        <svg
          viewBox={`0 0 ${svgWidth} ${svgHeight}`}
          className="conn-log-step-svg"
          preserveAspectRatio="none"
        >
          <defs>
            <linearGradient id="connStepGrad" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="#10b981" stopOpacity="0.45" />
              <stop offset="100%" stopColor="#10b981" stopOpacity="0.05" />
            </linearGradient>
          </defs>
          <line
            x1="0"
            y1={padding}
            x2={svgWidth}
            y2={padding}
            stroke="var(--border)"
            strokeDasharray="3 3"
            strokeWidth="1"
          />
          <line
            x1="0"
            y1={padding + chartHeight}
            x2={svgWidth}
            y2={padding + chartHeight}
            stroke="var(--border)"
            strokeWidth="1"
          />
          <path d={areaPath} fill="url(#connStepGrad)" />
          <path d={linePath} fill="none" stroke="#10b981" strokeWidth="2" strokeLinejoin="miter" />
        </svg>
        <div className="conn-log-time-axis">
          <span>{formatTimestamp(windowRange.start)}</span>
          <span>{formatShortTime(windowRange.start + totalMs * 0.5)}</span>
          <span>Now ({formatShortTime(windowRange.end)})</span>
        </div>
      </div>
    )
  }

  // Render Hourly Uptime Heatmap
  const renderHeatmap = (): ReactElement => {
    // Generate hourly buckets over the selected window
    const buckets: Array<{
      start: number
      end: number
      label: string
      uptimePct: number
      connectedMs: number
      totalMs: number
      disconnects: number
    }> = []

    const hourMs = 3600 * 1000
    const startHour = Math.floor(windowRange.start / hourMs) * hourMs
    const endHour = Math.ceil(windowRange.end / hourMs) * hourMs

    for (let t = startHour; t < endHour; t += hourMs) {
      const bStart = Math.max(windowRange.start, t)
      const bEnd = Math.min(windowRange.end, t + hourMs)
      const bDuration = Math.max(1, bEnd - bStart)

      // Calculate connected time in this bucket from spans
      let bConnMs = 0
      let bDrops = 0
      for (const span of spans) {
        const overlapStart = Math.max(bStart, span.start)
        const overlapEnd = Math.min(bEnd, span.end)
        if (overlapEnd > overlapStart) {
          if (span.kind === 'connected') {
            bConnMs += overlapEnd - overlapStart
          } else {
            bDrops++
          }
        }
      }

      const pct = Math.min(100, Math.max(0, (bConnMs / bDuration) * 100))
      buckets.push({
        start: bStart,
        end: bEnd,
        label: `${formatTimestamp(bStart)} - ${formatShortTime(bEnd)}`,
        uptimePct: pct,
        connectedMs: bConnMs,
        totalMs: bDuration,
        disconnects: bDrops
      })
    }

    return (
      <div className="conn-log-graph-box">
        <div className="conn-log-heatmap-grid">
          {buckets.map((b, i) => {
            let heatClass = 'heat-100'
            if (b.uptimePct === 0) heatClass = 'heat-0'
            else if (b.uptimePct < 50) heatClass = 'heat-low'
            else if (b.uptimePct < 85) heatClass = 'heat-mid'
            else if (b.uptimePct < 100) heatClass = 'heat-high'

            return (
              <div
                key={i}
                className={`conn-log-heat-cell ${heatClass}`}
                onMouseEnter={() => setHoveredCell(b)}
                onMouseLeave={() => setHoveredCell(null)}
              />
            )
          })}
        </div>
        <div className="conn-log-time-axis">
          <span>{formatTimestamp(windowRange.start)}</span>
          <div className="conn-log-heatmap-legend">
            <span>0%</span>
            <span className="heat-cell-sample heat-0" />
            <span className="heat-cell-sample heat-low" />
            <span className="heat-cell-sample heat-mid" />
            <span className="heat-cell-sample heat-high" />
            <span className="heat-cell-sample heat-100" />
            <span>100%</span>
          </div>
          <span>Now ({formatShortTime(windowRange.end)})</span>
        </div>
        {hoveredCell ? (
          <div className="conn-log-tooltip">
            <strong>{hoveredCell.label}</strong>
            <span>
              Uptime: {hoveredCell.uptimePct.toFixed(1)}% ({formatDuration(hoveredCell.connectedMs)} / {formatDuration(hoveredCell.totalMs)})
            </span>
            {hoveredCell.disconnects > 0 ? <span>{hoveredCell.disconnects} state change(s)</span> : null}
          </div>
        ) : null}
      </div>
    )
  }

  const statusToneClass =
    currentStatus === 'connected'
      ? 'status-connected'
      : currentStatus === 'reconnecting'
        ? 'status-reconnecting'
        : 'status-disconnected'

  return (
    <div className="conn-log-view">
      {/* Top Header / View Controls */}
      <div className="conn-log-controls-bar">
        <div className="conn-log-segmented-group" role="tablist" aria-label="Graph Mode">
          <button
            type="button"
            className={`conn-log-tab-btn${graphMode === 'timeline' ? ' active' : ''}`}
            onClick={() => setGraphMode('timeline')}
          >
            Timeline Bar
          </button>
          <button
            type="button"
            className={`conn-log-tab-btn${graphMode === 'step' ? ' active' : ''}`}
            onClick={() => setGraphMode('step')}
          >
            Step Chart
          </button>
          <button
            type="button"
            className={`conn-log-tab-btn${graphMode === 'heatmap' ? ' active' : ''}`}
            onClick={() => setGraphMode('heatmap')}
          >
            Uptime Heatmap
          </button>
        </div>

        <div className="conn-log-segmented-group" role="group" aria-label="Time Window">
          {(['1h', '6h', '24h', '7d', 'all'] as ConnectionLoggerTimeWindow[]).map((w) => (
            <button
              key={w}
              type="button"
              className={`conn-log-tab-btn${timeWindow === w ? ' active' : ''}`}
              onClick={() => setTimeWindow(w)}
            >
              {w.toUpperCase()}
            </button>
          ))}
        </div>

        <div className="conn-log-actions">
          <button type="button" className="conn-log-btn" onClick={exportCsv} title="Export event log as CSV">
            CSV
          </button>
          <button type="button" className="conn-log-btn" onClick={exportJson} title="Export event log as JSON">
            JSON
          </button>
          {showClearConfirm ? (
            <div className="conn-log-confirm-box">
              <span>Clear?</span>
              <button type="button" className="conn-log-btn conn-log-btn-danger" onClick={clearLogs}>
                Yes
              </button>
              <button type="button" className="conn-log-btn" onClick={() => setShowClearConfirm(false)}>
                No
              </button>
            </div>
          ) : (
            <button
              type="button"
              className="conn-log-btn"
              onClick={() => setShowClearConfirm(true)}
              title="Clear all stored logs for this host"
            >
              Clear
            </button>
          )}
        </div>
      </div>

      {/* Summary Metrics Bar */}
      <div className="conn-log-metrics-bar">
        <div className="conn-log-metric-card">
          <span className="metric-label">Uptime ({timeWindow.toUpperCase()})</span>
          <span className="metric-value metric-uptime">{stats.uptimePct.toFixed(1)}%</span>
        </div>
        <div className="conn-log-metric-card">
          <span className="metric-label">Current State</span>
          <span className={`metric-value ${statusToneClass}`}>
            ● {currentStatus.toUpperCase()}
            <small> ({formatDuration(stats.currentDurationMs)})</small>
          </span>
        </div>
        <div className="conn-log-metric-card">
          <span className="metric-label">Disconnects</span>
          <span className="metric-value">{stats.disconnectCount}</span>
        </div>
        <div className="conn-log-metric-card">
          <span className="metric-label">Total Connected</span>
          <span className="metric-value">{formatDuration(stats.connectedMs)}</span>
        </div>
        <div className="conn-log-metric-card">
          <span className="metric-label">Avg Session</span>
          <span className="metric-value">{formatDuration(stats.avgConnectedMs)}</span>
        </div>
      </div>

      {/* Selectable Graph View */}
      {graphMode === 'timeline' && renderTimelineBar()}
      {graphMode === 'step' && renderStepChart()}
      {graphMode === 'heatmap' && renderHeatmap()}

      {/* Filter and Search Bar */}
      <div className="conn-log-filter-bar">
        <div className="conn-log-search-wrap">
          <input
            type="text"
            className="conn-log-search-input"
            placeholder="Filter by message or details…"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
        </div>
        <div className="conn-log-filter-select-wrap">
          <select
            className="conn-log-filter-select"
            value={filterKind}
            onChange={(e) => setFilterKind(e.target.value)}
          >
            <option value="all">All Events ({events.length})</option>
            <option value="connected">Connected</option>
            <option value="disconnected">Disconnected</option>
            <option value="reconnecting">Reconnecting</option>
            <option value="failed">Failed</option>
          </select>
        </div>
        <span className="conn-log-count-text">
          Showing {filteredEvents.length} of {events.length}
        </span>
      </div>

      {/* Event Log Table */}
      <div className="conn-log-table-container">
        {filteredEvents.length === 0 ? (
          <div className="conn-log-empty">
            {events.length === 0
              ? 'No connection events recorded yet. Connect or disconnect this session to see events logged.'
              : 'No events match your current filter.'}
          </div>
        ) : (
          <table className="conn-log-table">
            <thead>
              <tr>
                <th>Timestamp</th>
                <th>Event</th>
                <th>State Duration</th>
                <th>Details / Reason</th>
              </tr>
            </thead>
            <tbody>
              {filteredEvents.map((e) => (
                <tr key={e.id}>
                  <td className="cell-timestamp">{formatTimestamp(e.timestamp)}</td>
                  <td className="cell-kind">
                    <span className={`conn-kind-badge badge-${e.kind}`}>{e.kind}</span>
                  </td>
                  <td className="cell-duration">{formatDuration(e.durationMs)}</td>
                  <td className="cell-message">{e.message || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}
