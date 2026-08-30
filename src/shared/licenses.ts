/** Direct runtime / bundled library attributions for the About dialog */
export interface ThirdPartyLicense {
  name: string
  license: string
  note?: string
}

export const THIRD_PARTY_LICENSES: ThirdPartyLicense[] = [
  { name: 'Electron', license: 'MIT' },
  { name: 'React', license: 'MIT' },
  { name: 'React DOM', license: 'MIT' },
  { name: 'xterm.js (@xterm/xterm)', license: 'MIT' },
  { name: '@xterm/addon-fit', license: 'MIT' },
  { name: '@xterm/addon-unicode11', license: 'MIT' },
  { name: '@xterm/addon-web-links', license: 'MIT' },
  { name: 'ssh2', license: 'MIT' },
  { name: 'mqtt.js', license: 'MIT' },
  { name: 'serialport', license: 'MIT' },
  { name: 'IBM Plex Mono (@fontsource/ibm-plex-mono)', license: 'OFL-1.1' },
  { name: 'JetBrains Mono (@fontsource/jetbrains-mono)', license: 'OFL-1.1' }
]
