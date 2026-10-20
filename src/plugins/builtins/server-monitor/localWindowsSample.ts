import { statfsSync } from 'node:fs'
import { cpus, freemem, hostname, networkInterfaces, release, totalmem, uptime } from 'node:os'

/** Build the fast-changing Windows host sample without spawning PowerShell. */
export function buildLocalWindowsSample(): string {
  const cpuList = cpus()
  const totals = { user: 0, nice: 0, sys: 0, idle: 0, irq: 0 }
  const cpuLines = cpuList.map((cpu, index) => {
    totals.user += cpu.times.user
    totals.nice += cpu.times.nice
    totals.sys += cpu.times.sys
    totals.idle += cpu.times.idle
    totals.irq += cpu.times.irq
    return `cpu${index} ${cpu.times.user} ${cpu.times.nice} ${cpu.times.sys} 0 ${cpu.times.idle} ${cpu.times.irq} 0 0`
  })
  const interfaces = networkInterfaces()
  const addresses = Object.entries(interfaces).flatMap(([name, entries]) =>
    (entries ?? [])
      .filter((entry) => !entry.internal)
      .map((entry) => ({ name, address: entry.address }))
  )
  const drive = process.env.SystemDrive || 'C:'
  const disk = statfsSync(`${drive}\\`)
  const diskTotal = disk.blocks * disk.bsize
  const diskUsed = (disk.blocks - disk.bfree) * disk.bsize
  const memoryTotalKb = Math.floor(totalmem() / 1024)
  const memoryAvailableKb = Math.floor(freemem() / 1024)
  const osVersion = `Windows ${release()}`

  return [
    '===META===',
    hostname(),
    String(Math.floor(uptime())),
    '0 0 0 0/0 0',
    osVersion,
    String(cpuList.length),
    '===IP===',
    ...addresses.map((entry) => entry.address),
    '===OS===',
    `PRETTY_NAME=${osVersion}`,
    '===CPU===',
    `cpu ${totals.user} ${totals.nice} ${totals.sys} 0 ${totals.idle} ${totals.irq} 0 0`,
    ...cpuLines,
    '===MEM===',
    `MemTotal: ${memoryTotalKb} kB`,
    `MemAvailable: ${memoryAvailableKb} kB`,
    `MemFree: ${memoryAvailableKb} kB`,
    '===DISK===',
    `${drive} ${diskTotal} ${diskUsed} 0`,
    '===DISKIO===',
    '===NET===',
    '===IPMAP===',
    ...addresses.map((entry) => `${entry.name} ${entry.address}`),
    '===NETSPEED===',
    '===TEMP===',
    '===PROCS==='
  ].join('\n')
}