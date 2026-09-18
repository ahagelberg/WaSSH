import { useState, type ReactElement } from 'react'
import type { PluginSettingsViewProps } from '@plugin-api/renderer'
import { MQTT_ANALYSER_NEW_MESSAGE_LABEL } from './defaults'
import type { MqttAnalyserSavedMessage } from './protocol'
import { normalizeSavedMessages, jsonParseError, SAVED_MESSAGE_MODES } from './viewUtils'
import './styles.css'

export default function MqttAnalyserSettings({
  settings,
  onSettingsPatch
}: PluginSettingsViewProps): ReactElement {
  const messages = normalizeSavedMessages(settings.messages)
  const [expandedId, setExpandedId] = useState<string | null>(null)

  const update = (next: MqttAnalyserSavedMessage[]): void => {
    onSettingsPatch({ messages: next })
  }

  const patchMessage = (id: string, partial: Partial<MqttAnalyserSavedMessage>): void => {
    update(messages.map((m) => (m.id === id ? { ...m, ...partial } : m)))
  }

  const addMessage = (): void => {
    const message: MqttAnalyserSavedMessage = {
      id: crypto.randomUUID(),
      label: MQTT_ANALYSER_NEW_MESSAGE_LABEL,
      topic: '',
      payload: '',
      mode: 'text'
    }
    update([...messages, message])
    setExpandedId(message.id)
  }

  return (
    <div className="mqtt-settings">
      {messages.length === 0 ? (
        <div className="mqtt-msg-empty">No saved messages</div>
      ) : (
        messages.map((message) => {
          const expanded = expandedId === message.id
          return (
            <div key={message.id} className="mqtt-settings-row">
              <div className="mqtt-settings-row-head">
                <button
                  type="button"
                  className="mqtt-msg-collapse"
                  aria-label={expanded ? 'Collapse message' : 'Expand message'}
                  aria-expanded={expanded}
                  onClick={() => setExpandedId(expanded ? null : message.id)}
                >
                  {expanded ? '▾' : '▸'}
                </button>
                <span className="mqtt-settings-row-label">
                  {message.label || 'Untitled'}
                </span>
                <button
                  type="button"
                  className="danger"
                  onClick={() => update(messages.filter((m) => m.id !== message.id))}
                >
                  Remove
                </button>
              </div>
              {expanded ? (
                <div className="mqtt-settings-row-body">
                  <label className="mqtt-field">
                    <span>Label</span>
                    <input
                      type="text"
                      value={message.label}
                      onChange={(e) => patchMessage(message.id, { label: e.target.value })}
                    />
                  </label>
                  <label className="mqtt-field">
                    <span>Topic</span>
                    <input
                      type="text"
                      value={message.topic}
                      spellCheck={false}
                      onChange={(e) => patchMessage(message.id, { topic: e.target.value })}
                    />
                  </label>
                  <div className="mqtt-publish-modes">
                    {SAVED_MESSAGE_MODES.map((m) => (
                      <button
                        key={m}
                        type="button"
                        className={message.mode === m ? 'active' : ''}
                        onClick={() => patchMessage(message.id, { mode: m })}
                      >
                        {m === 'json' ? 'JSON' : 'Text'}
                      </button>
                    ))}
                  </div>
                  <label className="mqtt-field">
                    <span>Payload</span>
                    <textarea
                      rows={4}
                      value={message.payload}
                      spellCheck={false}
                      aria-invalid={message.mode === 'json' && jsonParseError(message.payload) !== null}
                      onChange={(e) => patchMessage(message.id, { payload: e.target.value })}
                    />
                  </label>
                  {message.mode === 'json' && jsonParseError(message.payload) !== null ? (
                    <div className="mqtt-msg-dialog-error">{jsonParseError(message.payload)}</div>
                  ) : null}
                </div>
              ) : null}
            </div>
          )
        })
      )}
      <button type="button" onClick={addMessage}>
        Add message
      </button>
    </div>
  )
}
