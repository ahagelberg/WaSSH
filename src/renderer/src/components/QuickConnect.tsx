import { useState } from 'react'
import {
  CONNECTION_TYPE_SERIAL,
  CONNECTION_TYPE_SSH,
  CONNECTION_TYPE_TELNET,
  DEFAULT_CONNECTION_TYPE,
  DEFAULT_SERIAL_DATA_BITS,
  DEFAULT_SERIAL_FLOW_CONTROL,
  DEFAULT_SERIAL_PARITY,
  DEFAULT_SERIAL_STOP_BITS,
  SERIAL_BAUD_MAX,
  SERIAL_BAUD_MIN,
  SERIAL_BAUD_RATES,
  type ConnectionParams,
  type ConnectionType,
  type SerialDataBits,
  type SerialFlowControl,
  type SerialParity,
  type SerialStopBits
} from '@shared/types'
import {
  defaultPortForType,
  emptyConnectionParams,
  protocolConfigFrom
} from '@shared/connection'
import SerialPortField from './SerialPortField'
import {
  SERIAL_DATA_BITS_OPTIONS,
  SERIAL_PARITY_OPTIONS,
  SERIAL_STOP_BITS_OPTIONS,
  serialFlowOptions
} from './serialOptions'

interface Props {
  onConnect: (connection: ConnectionParams) => void
}

function quickName(
  type: ConnectionType,
  host: string,
  username: string
): string {
  if (type === CONNECTION_TYPE_SERIAL) {
    return host
  }
  if (username) {
    return `${username}@${host}`
  }
  return host
}

export default function QuickConnect({ onConnect }: Props) {
  const [type, setType] = useState<ConnectionType>(DEFAULT_CONNECTION_TYPE)
  const [host, setHost] = useState('')
  const [port, setPort] = useState(String(defaultPortForType(DEFAULT_CONNECTION_TYPE)))
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [baud, setBaud] = useState(String(protocolConfigFrom(null).serialBaudRate))
  const [dataBits, setDataBits] = useState<SerialDataBits>(DEFAULT_SERIAL_DATA_BITS)
  const [parity, setParity] = useState<SerialParity>(DEFAULT_SERIAL_PARITY)
  const [stopBits, setStopBits] = useState<SerialStopBits>(DEFAULT_SERIAL_STOP_BITS)
  const [flow, setFlow] = useState<SerialFlowControl>(DEFAULT_SERIAL_FLOW_CONTROL)

  const changeType = (next: ConnectionType): void => {
    const prevDefault = defaultPortForType(type)
    const nextDefault = defaultPortForType(next)
    if (Number(port) === prevDefault || port === '') {
      setPort(nextDefault ? String(nextDefault) : '')
    }
    setType(next)
  }

  const submit = (): void => {
    const trimmed = host.trim()
    if (!trimmed) {
      return
    }
    const serial = protocolConfigFrom({
      connectionType: type,
      serialBaudRate: Number(baud),
      serialDataBits: dataBits,
      serialStopBits: stopBits,
      serialParity: parity,
      serialFlowControl: flow
    })
    onConnect({
      ...emptyConnectionParams(type),
      name: quickName(type, trimmed, username.trim()),
      host: trimmed,
      port: type === CONNECTION_TYPE_SERIAL ? 0 : Number(port) || defaultPortForType(type),
      username: type === CONNECTION_TYPE_SSH ? username.trim() : '',
      authMethod: type === CONNECTION_TYPE_SSH && password ? 'password' : 'none',
      ...serial,
      ephemeralPassword: type === CONNECTION_TYPE_SSH ? password : ''
    })
  }

  return (
    <div className="quick-connect">
      <h3>Quick connect</h3>
      <div className="field-row">
        <label htmlFor="qc-type">Type</label>
        <select
          id="qc-type"
          value={type}
          onChange={(e) => changeType(e.target.value as ConnectionType)}
        >
          <option value={CONNECTION_TYPE_SSH}>SSH</option>
          <option value={CONNECTION_TYPE_TELNET}>Telnet</option>
          <option value={CONNECTION_TYPE_SERIAL}>Serial</option>
        </select>
      </div>
      {type === CONNECTION_TYPE_SERIAL ? (
        <>
          <div className="field-row">
            <label htmlFor="qc-serial-port">Port</label>
            <SerialPortField
              id="qc-serial-port"
              listId="qc-serial-ports"
              value={host}
              onChange={setHost}
              onSubmit={submit}
            />
          </div>
          <div className="field-row">
            <label htmlFor="qc-baud">Baud</label>
            <input
              id="qc-baud"
              type="number"
              list="qc-baud-rates"
              min={SERIAL_BAUD_MIN}
              max={SERIAL_BAUD_MAX}
              value={baud}
              onChange={(e) => setBaud(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && submit()}
            />
            <datalist id="qc-baud-rates">
              {SERIAL_BAUD_RATES.map((rate) => (
                <option key={rate} value={rate} />
              ))}
            </datalist>
          </div>
          <div className="quick-connect-serial-format">
            <div className="field-row">
              <label htmlFor="qc-databits">Data bits</label>
              <select
                id="qc-databits"
                value={dataBits}
                onChange={(e) => setDataBits(Number(e.target.value) as SerialDataBits)}
              >
                {SERIAL_DATA_BITS_OPTIONS}
              </select>
            </div>
            <div className="field-row">
              <label htmlFor="qc-parity">Parity</label>
              <select
                id="qc-parity"
                value={parity}
                onChange={(e) => setParity(e.target.value as SerialParity)}
              >
                {SERIAL_PARITY_OPTIONS}
              </select>
            </div>
            <div className="field-row">
              <label htmlFor="qc-stopbits">Stop bits</label>
              <select
                id="qc-stopbits"
                value={stopBits}
                onChange={(e) => setStopBits(Number(e.target.value) as SerialStopBits)}
              >
                {SERIAL_STOP_BITS_OPTIONS}
              </select>
            </div>
            <div className="field-row">
              <label htmlFor="qc-flow">Flow</label>
              <select
                id="qc-flow"
                value={flow}
                onChange={(e) => setFlow(e.target.value as SerialFlowControl)}
              >
                {serialFlowOptions(false)}
              </select>
            </div>
          </div>
        </>
      ) : (
        <>
          <div className="field-row">
            <label htmlFor="qc-host">Host</label>
            <div className="quick-connect-row">
              <input
                id="qc-host"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && submit()}
                placeholder="hostname"
              />
              <input
                aria-label="Port"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && submit()}
              />
            </div>
          </div>
          {type === CONNECTION_TYPE_SSH ? (
            <>
              <div className="field-row">
                <label htmlFor="qc-user">Username</label>
                <input
                  id="qc-user"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && submit()}
                />
              </div>
              <div className="field-row">
                <label htmlFor="qc-pass">Password</label>
                <input
                  id="qc-pass"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && submit()}
                  placeholder="optional"
                />
              </div>
            </>
          ) : null}
        </>
      )}
      <button type="button" className="primary" onClick={submit}>
        Connect
      </button>
    </div>
  )
}
