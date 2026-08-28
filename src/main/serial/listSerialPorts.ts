import type { SerialPortInfo } from '../../shared/types'

export async function listSerialPorts(): Promise<SerialPortInfo[]> {
  try {
    const { SerialPort } = await import('serialport')
    const ports = await SerialPort.list()
    return ports
      .filter((p) => Boolean(p.path))
      .map((p) => {
        const extra = p as { friendlyName?: string }
        const detail = [extra.friendlyName, p.manufacturer].filter(Boolean).join(' · ')
        return { path: p.path, detail }
      })
  } catch {
    return []
  }
}
