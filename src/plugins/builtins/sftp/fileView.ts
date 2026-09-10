import type { PluginMainContext, SftpSession } from '../../../main/plugins/api'
import { classifySftpError } from '../../../main/plugins/api'
import type { SftpViewFilePayload } from './protocol'

const SFTP_VIEW_MAX_BYTES = 1024 * 1024

function sendViewFileError(
  ctx: PluginMainContext,
  path: string,
  error: ReturnType<typeof classifySftpError>
): void {
  ctx.sendToRenderer({
    type: 'viewFileResult',
    path,
    ok: false,
    bytesRead: 0,
    truncated: false,
    error: error.message,
    errorKind: error.kind
  } satisfies SftpViewFilePayload)
}

/**
 * Total-Commander-style sniff: text when no unprintable control bytes appear
 * in the fetched bytes (tab, LF, FF and CR are tolerated). High-bit bytes are
 * allowed so UTF-8 / latin-1 text stays readable.
 */
function isLikelyTextView(buf: Buffer): boolean {
  for (const b of buf) {
    if (b === 0x09 || b === 0x0a || b === 0x0c || b === 0x0d) {
      continue
    }
    if (b < 0x20 || b === 0x7f) {
      return false
    }
  }
  return true
}

/** Best-effort text decode: UTF-8 when valid, otherwise byte-per-char (latin-1). */
function decodeViewText(buf: Buffer): string {
  try {
    const utf8 = buf.toString('utf8')
    if (Buffer.from(utf8, 'utf8').equals(buf)) {
      return utf8
    }
  } catch {
    /* fall through to single-byte decode */
  }
  return buf.toString('latin1')
}

function readViewBytes(sftp: SftpSession, path: string): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const remote = sftp.createReadStream(path)
    const chunks: Buffer[] = []
    let received = 0
    let settled = false
    const finish = (err?: Error): void => {
      if (settled) {
        return
      }
      settled = true
      try {
        remote.destroy()
      } catch {
        /* ignore */
      }
      if (err) {
        reject(classifySftpError(err))
      } else {
        resolve(Buffer.concat(chunks, received))
      }
    }
    remote.on('error', (err: Error) => finish(err))
    remote.on('end', () => finish())
    remote.on('data', (chunk: Buffer) => {
      if (settled) {
        return
      }
      const remaining = SFTP_VIEW_MAX_BYTES - received
      const take = chunk.length <= remaining ? chunk : chunk.subarray(0, remaining)
      chunks.push(take)
      received += take.length
      if (received >= SFTP_VIEW_MAX_BYTES) {
        finish()
      }
    })
  })
}

export async function handleViewFile(
  ctx: PluginMainContext,
  sftp: SftpSession | null,
  path: string
): Promise<void> {
  if (!sftp) {
    sendViewFileError(ctx, path, { message: 'SFTP session is not connected', kind: 'connection' })
    return
  }

  let totalBytes = 0
  try {
    totalBytes = (await sftp.stat(path)).size
  } catch {
    totalBytes = 0
  }

  let buf: Buffer
  try {
    buf = await readViewBytes(sftp, path)
  } catch (err) {
    sendViewFileError(ctx, path, classifySftpError(err))
    return
  }

  const truncated =
    totalBytes > 0 ? buf.length < totalBytes : buf.length >= SFTP_VIEW_MAX_BYTES
  if (isLikelyTextView(buf)) {
    ctx.sendToRenderer({
      type: 'viewFileResult',
      path,
      ok: true,
      kind: 'text',
      text: decodeViewText(buf),
      bytesRead: buf.length,
      totalBytes,
      truncated
    } satisfies SftpViewFilePayload)
  } else {
    ctx.sendToRenderer({
      type: 'viewFileResult',
      path,
      ok: true,
      kind: 'binary',
      contentBase64: buf.toString('base64'),
      bytesRead: buf.length,
      totalBytes,
      truncated
    } satisfies SftpViewFilePayload)
  }
}
