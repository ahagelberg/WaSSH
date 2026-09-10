import type { ReactElement } from 'react'
import type { PluginSettingsField } from '@plugin-api/shared'
import PluginFieldEditor from './PluginFieldEditor'

interface Props {
  schema: PluginSettingsField[]
  values: Record<string, unknown>
  onChange: (key: string, value: unknown) => void
  /** Nesting depth; 0 = top-level fields in the schema, incremented per `group` level. */
  depth?: number
}

/** Caps the compactness tier so arbitrarily deep nesting doesn't keep shrinking rows further. */
const MAX_COMPACT_DEPTH = 2

/**
 * Recursively renders a plugin settings schema: one row per field, and for
 * `type: 'group'` fields, an indented block of child rows that stays visible
 * but disabled while the group's own master toggle is off. Rows below the
 * top level render more compactly (smaller padding/text), one extra notch
 * per depth up to `MAX_COMPACT_DEPTH`, so deeply nested option trees (e.g.
 * Permissions > category > method) stay scannable.
 */
export default function PluginSettingsFieldList({ schema, values, onChange, depth = 0 }: Props): ReactElement {
  const tier = Math.min(depth, MAX_COMPACT_DEPTH)
  const rowClassName = depth > 0 ? `settings-row plugin-settings-row-nested depth-${tier}` : 'settings-row'
  return (
    <>
      {schema.map((field) => {
        if (field.type === 'group') {
          const enabled = Boolean(values[field.key])
          const childTier = Math.min(depth + 1, MAX_COMPACT_DEPTH)
          return (
            <div key={field.key} className="plugin-settings-group">
              <div className={rowClassName}>
                <div className="settings-row-label">
                  <strong>{field.label}</strong>
                  {field.description ? <span>{field.description}</span> : null}
                </div>
                <PluginFieldEditor
                  field={field}
                  value={values[field.key]}
                  onChange={(value) => onChange(field.key, value)}
                />
              </div>
              <div
                className={`plugin-settings-group-children depth-${childTier}${enabled ? '' : ' is-disabled'}`}
                aria-disabled={!enabled}
              >
                <PluginSettingsFieldList
                  schema={field.children ?? []}
                  values={values}
                  onChange={onChange}
                  depth={depth + 1}
                />
              </div>
            </div>
          )
        }
        return (
          <div key={field.key} className={rowClassName}>
            <div className="settings-row-label">
              <strong>{field.label}</strong>
              {field.description ? <span>{field.description}</span> : null}
            </div>
            <PluginFieldEditor
              field={field}
              value={values[field.key]}
              onChange={(value) => onChange(field.key, value)}
            />
          </div>
        )
      })}
    </>
  )
}
