import ColorHexInput from './ColorHexInput'
import { HEX_COLOR_RE, themeVarHex } from './settingsColor'

interface Props {
  label: string
  hint: string
  value: string
  themeVar: string
  /** Label for the "no custom color" checkbox */
  defaultLabel: string
  /** Hex used when turning off the default if the theme var is not a hex; defaults to the theme var hex */
  resolvedFallback?: string
  onChange: (value: string) => void
}

/** Color row with a "use default" checkbox, swatch picker, and hex input. */
export default function SettingsColorRow({
  label,
  hint,
  value,
  themeVar,
  defaultLabel,
  resolvedFallback,
  onChange
}: Props) {
  const useDefault = !value
  const fallback = resolvedFallback ?? themeVarHex(themeVar)
  const pickerValue = useDefault
    ? HEX_COLOR_RE.test(fallback)
      ? fallback
      : themeVarHex(themeVar)
    : value
  return (
    <div className="settings-row">
      <div className="settings-row-label">
        <strong>{label}</strong>
        <span>{hint}</span>
      </div>
      <div className="settings-color-field">
        <label className="settings-theme-default">
          <input
            type="checkbox"
            checked={useDefault}
            onChange={(e) =>
              onChange(e.target.checked ? '' : HEX_COLOR_RE.test(fallback) ? fallback : themeVarHex(themeVar))
            }
          />
          {defaultLabel}
        </label>
        {HEX_COLOR_RE.test(pickerValue) ? (
          <>
            <input
              type="color"
              value={pickerValue}
              onClick={() => {
                // Clicking the swatch starts a custom color: clear the default
                // even if the picker is cancelled.
                if (useDefault) {
                  onChange(HEX_COLOR_RE.test(fallback) ? fallback : themeVarHex(themeVar))
                }
              }}
              onChange={(e) => onChange(e.target.value)}
            />
            <ColorHexInput value={pickerValue} onChange={onChange} />
          </>
        ) : null}
      </div>
    </div>
  )
}
