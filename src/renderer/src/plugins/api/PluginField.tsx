import type { ReactNode } from 'react'

interface Props {
  label: ReactNode
  hint?: ReactNode
  children: ReactNode
  className?: string
}

export default function PluginField({ label, hint, children, className = '' }: Props) {
  return (
    <div className={['plugin-ui-field', className].filter(Boolean).join(' ')}>
      <label>{label}</label>
      {children}
      {hint ? <span className="plugin-ui-hint">{hint}</span> : null}
    </div>
  )
}
