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
  SERIAL_DATA_BITS_5,
  SERIAL_DATA_BITS_6,
  SERIAL_DATA_BITS_7,
  SERIAL_DATA_BITS_8,
  SERIAL_FLOW_NONE,
  SERIAL_FLOW_RTSCTS,
  SERIAL_FLOW_XONXOFF,
  SERIAL_PARITY_EVEN,
  SERIAL_PARITY_MARK,
  SERIAL_PARITY_NONE,
  SERIAL_PARITY_ODD,
  SERIAL_PARITY_SPACE,
  SERIAL_STOP_BITS_1,
  SERIAL_STOP_BITS_1_5,
  SERIAL_STOP_BITS_2,
  type ConnectionParams,
  type ConnectionType,
  type SerialDataBits,
  type SerialFlowControl,
  type SerialParity,
  type SerialStopBits
} from '@shared/types'
import {
  defaultPortForType,
  protocolConfigFrom,
  reconnectModeFrom,
  sessionStyleFrom,
  tunnelConfigFrom
} from '@shared/connection'
import SerialPortField from './SerialPortField'

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
      hostId: null,
      name: quickName(type, trimmed, username.trim()),
      host: trimmed,
      port: type === CONNECTION_TYPE_SERIAL ? 0 : Number(port) || defaultPortForType(type),
      username: type === CONNECTION_TYPE_SSH ? username.trim() : '',
      passwordVaultId: '',
      privateKeyPath: '',
      passphraseVaultId: '',
      authMethod: type === CONNECTION_TYPE_SSH && password ? 'password' : 'none',
      proxyHostId: '',
      ...serial,
      ...sessionStyleFrom(null),
      ...tunnelConfigFrom(null),
      ephemeralPassword: type === CONNECTION_TYPE_SSH ? password : '',
      ephemeralPassphrase: '',
      pluginSettings: {},
      reconnectMode: reconnectModeFrom(null)
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
                <option value={SERIAL_DATA_BITS_5}>5</option>
                <option value={SERIAL_DATA_BITS_6}>6</option>
                <option value={SERIAL_DATA_BITS_7}>7</option>
                <option value={SERIAL_DATA_BITS_8}>8</option>
              </select>
            </div>
            <div className="field-row">
              <label htmlFor="qc-parity">Parity</label>
              <select
                id="qc-parity"
                value={parity}
                onChange={(e) => setParity(e.target.value as SerialParity)}
              >
                <option value={SERIAL_PARITY_NONE}>None</option>
                <option value={SERIAL_PARITY_EVEN}>Even</option>
                <option value={SERIAL_PARITY_ODD}>Odd</option>
                <option value={SERIAL_PARITY_MARK}>Mark</option>
                <option value={SERIAL_PARITY_SPACE}>Space</option>
              </select>
            </div>
            <div className="field-row">
              <label htmlFor="qc-stopbits">Stop bits</label>
              <select
                id="qc-stopbits"
                value={stopBits}
                onChange={(e) => setStopBits(Number(e.target.value) as SerialStopBits)}
              >
                <option value={SERIAL_STOP_BITS_1}>1</option>
                <option value={SERIAL_STOP_BITS_1_5}>1.5</option>
                <option value={SERIAL_STOP_BITS_2}>2</option>
              </select>
            </div>
            <div className="field-row">
              <label htmlFor="qc-flow">Flow</label>
              <select
                id="qc-flow"
                value={flow}
                onChange={(e) => setFlow(e.target.value as SerialFlowControl)}
              >
                <option value={SERIAL_FLOW_NONE}>None</option>
                <option value={SERIAL_FLOW_RTSCTS}>RTS/CTS</option>
                <option value={SERIAL_FLOW_XONXOFF}>XON/XOFF</option>
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
