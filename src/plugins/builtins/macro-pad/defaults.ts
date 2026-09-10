import type { PluginCommand } from '@plugin-api/shared'

export const DEFAULT_MACRO_BUTTONS: PluginCommand[] = [
  {
    id: 'm1',
    label: 'syslog',
    text: 'tail -n 100 /var/log/syslog\n',
    hotkey: ''
  },
  {
    id: 'm2',
    label: 'apt upgrade',
    text: 'sudo apt update;sudo apt upgrade -y;sudo apt autoremove -y\n',
    hotkey: ''
  },
  {
    id: 'm3',
    label: 'create venv',
    text: 'python3 -m venv .venv\n',
    hotkey: ''
  },
  {
    id: 'm4',
    label: 'ps aux',
    text: 'ps aux\n',
    hotkey: ''
  }
]

export const MACRO_PAD_UNGROUPED_COLLAPSED_DEFAULT = false
