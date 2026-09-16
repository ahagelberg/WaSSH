import type { SerialPortInfo } from '../../shared/types'

/** Windows-only driver info that serialport's typings omit */
interface WindowsPortInfo {
  friendlyName?: string
}

export async function listSerialPorts(): Promise<SerialPortInfo[]> {
  try {
    const { SerialPort } = await import('serialport')
    const ports = await SerialPort.list()
    return ports
      .filter((p) => Boolean(p.path))
      .map((p) => {
        const extra = p as WindowsPortInfo
        const detail = [extra.friendlyName, p.manufacturer].filter(Boolean).join(' · ')
        return { path: p.path, detail }
      })
  } catch (err) {
    console.error('Failed to list serial ports:', err)
    return []
  }
}
