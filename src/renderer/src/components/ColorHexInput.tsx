import { PluginColorInput } from '../plugins/api'

interface Props {
  value: string
  onChange: (hex: string) => void
}

export default function ColorHexInput(props: Props) {
  return <PluginColorInput {...props} className="settings-color-hex" />
}
