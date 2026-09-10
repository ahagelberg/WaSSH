import type { ButtonHTMLAttributes, ReactNode } from 'react'

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  children: ReactNode
  variant?: 'default' | 'primary' | 'danger'
  compact?: boolean
}

export default function PluginButton({
  children,
  variant = 'default',
  compact = false,
  className = '',
  type = 'button',
  ...props
}: Props) {
  const classes = [
    'plugin-ui-button',
    variant === 'default' ? '' : `plugin-ui-button--${variant}`,
    compact ? 'plugin-ui-button--compact' : '',
    className
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <button {...props} type={type} className={classes}>
      {children}
    </button>
  )
}
