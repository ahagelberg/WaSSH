import type { ReactElement } from 'react'
import type { PluginMacroButton, PluginSettingsField } from '@shared/plugins'

function MacroListEditor({
  value,
  onChange
}: {
  value: PluginMacroButton[]
  onChange: (next: PluginMacroButton[]) => void
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

export default function PluginFieldEditor({
  field,
  value,
  onChange
}: {
  field: PluginSettingsField
  value: unknown
  onChange: (value: unknown) => void
}): ReactElement {
  if (field.type === 'boolean') {
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
  if (field.type === 'macroList') {
    const list = Array.isArray(value) ? (value as PluginMacroButton[]) : []
    return <MacroListEditor value={list} onChange={onChange} />
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
  return (
    <input
      type={field.secret ? 'password' : 'text'}
      value={typeof value === 'string' ? value : String(value ?? '')}
      onChange={(e) => onChange(e.target.value)}
      autoComplete={field.secret ? 'off' : undefined}
    />
  )
}
