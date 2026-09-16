import type { ReactElement } from 'react'
import {
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
  SERIAL_STOP_BITS_2
} from '@shared/types'

/** `<option>` list for serial data bits. */
export const SERIAL_DATA_BITS_OPTIONS = (
  <>
    <option value={SERIAL_DATA_BITS_5}>5</option>
    <option value={SERIAL_DATA_BITS_6}>6</option>
    <option value={SERIAL_DATA_BITS_7}>7</option>
    <option value={SERIAL_DATA_BITS_8}>8</option>
  </>
)

/** `<option>` list for serial parity. */
export const SERIAL_PARITY_OPTIONS = (
  <>
    <option value={SERIAL_PARITY_NONE}>None</option>
    <option value={SERIAL_PARITY_EVEN}>Even</option>
    <option value={SERIAL_PARITY_ODD}>Odd</option>
    <option value={SERIAL_PARITY_MARK}>Mark</option>
    <option value={SERIAL_PARITY_SPACE}>Space</option>
  </>
)

/** `<option>` list for serial stop bits. */
export const SERIAL_STOP_BITS_OPTIONS = (
  <>
    <option value={SERIAL_STOP_BITS_1}>1</option>
    <option value={SERIAL_STOP_BITS_1_5}>1.5</option>
    <option value={SERIAL_STOP_BITS_2}>2</option>
  </>
)

/**
 * `<option>` list for serial flow control.
 * `verbose` uses the long labels shown in the host settings dialog.
 */
export function serialFlowOptions(verbose: boolean): ReactElement {
  return (
    <>
      <option value={SERIAL_FLOW_NONE}>None</option>
      <option value={SERIAL_FLOW_RTSCTS}>
        {verbose ? 'Hardware (RTS/CTS)' : 'RTS/CTS'}
      </option>
      <option value={SERIAL_FLOW_XONXOFF}>
        {verbose ? 'Software (XON/XOFF)' : 'XON/XOFF'}
      </option>
    </>
  )
}
