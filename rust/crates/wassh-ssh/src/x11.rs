//! X11 forwarding.
//!
//! With `x11-req` accepted, the server opens an `x11` channel per remote client. We accept
//! each channel and bridge it to the local X server, which gives the remote host a working
//! `DISPLAY` without exposing a listener: a Unix socket on Unix, loopback TCP on Windows.
//!
//! Authentication uses the MIT-MAGIC-COOKIE-1 entry from `~/.Xauthority` (or `XAUTHORITY`)
//! for the current display, sent hex-encoded exactly as OpenSSH does.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

/// Display `n` listens on TCP port `6000 + n`.
const X11_TCP_PORT_BASE: u16 = 6000;
/// Host used when `DISPLAY` names none.
const DEFAULT_X11_HOST: &str = "127.0.0.1";
/// Display assumed on Windows when `DISPLAY` is unset.
const DEFAULT_DISPLAY_NUMBER: u32 = 0;

/// Everything the `x11-req` packet needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11Config {
    /// Host part of `DISPLAY` (`""` for `:0`). Windows reaches the X server over TCP at
    /// this host; Unix always bridges to the local socket.
    pub host: String,
    /// Local display number (`:0` → 0).
    pub display_number: u32,
    /// Screen number after the dot (`:0.1` → 1).
    pub screen: u32,
    pub protocol: String,
    /// Hex-encoded authentication cookie, as `x11-req` expects.
    pub cookie_hex: String,
}

/// Parse `DISPLAY` into `(display number, screen)`.
///
/// Accepts `:0`, `:10.0`, `localhost:10.0`, `unix:0` and rejects anything without a
/// display number.
pub fn parse_display(display: &str) -> Option<(u32, u32)> {
    let after_colon = display.rsplit_once(':')?.1;
    if after_colon.is_empty() {
        return None;
    }
    let (number, screen) = match after_colon.split_once('.') {
        Some((number, screen)) => (number, screen.parse::<u32>().ok()?),
        None => (after_colon, 0),
    };
    Some((number.parse::<u32>().ok()?, screen))
}

/// Host part of a `DISPLAY` value (`""` for `:0`).
fn display_host(display: &str) -> &str {
    display
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or_default()
}

/// Read the MIT-MAGIC-COOKIE-1 entry for `display_number` from an Xauthority file.
///
/// The file is a sequence of big-endian records:
/// `family, address, number, name, data`, each length-prefixed with a `u16`.
pub fn read_cookie(path: &Path, display_number: u32) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    read_cookie_from(&bytes, display_number)
}

/// Same as [`read_cookie`], for an in-memory file.
pub fn read_cookie_from(bytes: &[u8], display_number: u32) -> Option<Vec<u8>> {
    let wanted_number = display_number.to_string();
    let mut cursor = 0usize;

    while cursor + 2 <= bytes.len() {
        let _family = read_u16(bytes, &mut cursor)?;
        let address = read_blob(bytes, &mut cursor)?;
        let number = read_blob(bytes, &mut cursor)?;
        let name = read_blob(bytes, &mut cursor)?;
        let data = read_blob(bytes, &mut cursor)?;

        let _ = address;
        let number = String::from_utf8_lossy(number);
        let name = String::from_utf8_lossy(name);
        if number == wanted_number && name == "MIT-MAGIC-COOKIE-1" {
            return Some(data.to_vec());
        }
    }
    None
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let end = cursor.checked_add(2)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_blob<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let length = usize::from(read_u16(bytes, cursor)?);
    let end = cursor.checked_add(length)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(slice)
}

/// Locate the current display and its cookie, or `None` when there is no X server.
pub fn local_config() -> Option<X11Config> {
    let (home, display_number, screen) = local_display()?;
    let cookie = authority_path()
        .and_then(|path| read_cookie(&path, display_number))
        .unwrap_or_default();

    Some(X11Config {
        host: home,
        display_number,
        screen,
        protocol: "MIT-MAGIC-COOKIE-1".to_owned(),
        cookie_hex: hex(&cookie),
    })
}

/// Host, display and screen of the local X server.
#[cfg(unix)]
fn local_display() -> Option<(String, u32, u32)> {
    let display = std::env::var("DISPLAY").ok()?;
    let (display_number, screen) = parse_display(&display)?;
    Some((display_host(&display).to_owned(), display_number, screen))
}

/// Windows X servers listen on TCP and announce themselves through no standard variable, so
/// an unset `DISPLAY` means display 0 on this machine.
#[cfg(windows)]
fn local_display() -> Option<(String, u32, u32)> {
    let Ok(display) = std::env::var("DISPLAY") else {
        return Some((String::new(), DEFAULT_DISPLAY_NUMBER, 0));
    };
    let (display_number, screen) = parse_display(&display)?;
    Some((display_host(&display).to_owned(), display_number, screen))
}

/// Unix looks in `XAUTHORITY`, then `~/.Xauthority`; Windows has no standard location.
#[cfg(unix)]
fn authority_path() -> Option<PathBuf> {
    std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")))
}

#[cfg(windows)]
fn authority_path() -> Option<PathBuf> {
    std::env::var_os("XAUTHORITY").map(PathBuf::from)
}

/// Lowercase hex, as used for the `x11-req` cookie field.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Path of the local X server socket for a display number.
#[cfg(unix)]
pub fn socket_path(display_number: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/.X11-unix/X{display_number}"))
}

/// Bridge one SSH X11 channel to the local X server socket until either side closes.
#[cfg(unix)]
pub async fn bridge(
    channel: russh::Channel<russh::client::Msg>,
    config: &X11Config,
) -> Result<()> {
    let path = socket_path(config.display_number);
    let server = UnixStream::connect(&path)
        .await
        .with_context(|| format!("connecting to the local X server at {}", path.display()))?;
    splice(channel, server).await
}

/// Bridge one SSH X11 channel to the local X server over TCP until either side closes.
#[cfg(windows)]
pub async fn bridge(
    channel: russh::Channel<russh::client::Msg>,
    config: &X11Config,
) -> Result<()> {
    let address = x11_address(config);
    let server = TcpStream::connect(&address)
        .await
        .with_context(|| format!("connecting to the local X server at {address}"))?;
    splice(channel, server).await
}

/// TCP address of the local X server.
#[cfg(windows)]
fn x11_address(config: &X11Config) -> String {
    let host = match config.host.as_str() {
        "" => DEFAULT_X11_HOST,
        host => host,
    };
    let port = u32::from(X11_TCP_PORT_BASE) + config.display_number;
    format!("{host}:{port}")
}

/// Copy bytes between the SSH channel and the local X server until either side closes.
async fn splice<S: AsyncRead + AsyncWrite + Unpin>(
    channel: russh::Channel<russh::client::Msg>,
    mut server: S,
) -> Result<()> {
    let mut channel = channel.into_stream();
    tokio::io::copy_bidirectional(&mut channel, &mut server)
        .await
        .map_err(|error| anyhow!("X11 channel closed: {error}"))?;
    let _ = channel.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(address: &str, number: &str, name: &str, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0xffffu16.to_be_bytes()); // FamilyWild
        for blob in [address.as_bytes(), number.as_bytes(), name.as_bytes(), data] {
            out.extend_from_slice(&(blob.len() as u16).to_be_bytes());
            out.extend_from_slice(blob);
        }
        out
    }

    #[test]
    fn parses_displays() {
        assert_eq!(parse_display(":0"), Some((0, 0)));
        assert_eq!(parse_display(":10.1"), Some((10, 1)));
        assert_eq!(parse_display("localhost:10.0"), Some((10, 0)));
        assert_eq!(parse_display("unix:0"), Some((0, 0)));
        assert_eq!(parse_display("nonsense"), None);
        assert_eq!(parse_display("host:"), None);
        assert_eq!(parse_display(":abc"), None);
    }

    #[test]
    fn splits_the_display_host() {
        assert_eq!(display_host(":0"), "");
        assert_eq!(display_host("127.0.0.1:10.0"), "127.0.0.1");
    }

    #[cfg(windows)]
    #[test]
    fn tcp_address_uses_port_6000_plus_the_display() {
        let local = X11Config {
            host: String::new(),
            display_number: 0,
            screen: 0,
            protocol: "MIT-MAGIC-COOKIE-1".to_owned(),
            cookie_hex: String::new(),
        };
        assert_eq!(x11_address(&local), "127.0.0.1:6000");
        let remote = X11Config {
            host: "192.0.2.10".to_owned(),
            display_number: 10,
            ..local
        };
        assert_eq!(x11_address(&remote), "192.0.2.10:6010");
    }

    #[test]
    fn finds_the_cookie_for_the_matching_display() {
        let mut file = record("", "0", "MIT-MAGIC-COOKIE-1", &[1, 2, 3, 4]);
        file.extend_from_slice(&record("", "10", "MIT-MAGIC-COOKIE-1", &[9, 9, 9, 9]));
        assert_eq!(read_cookie_from(&file, 0), Some(vec![1, 2, 3, 4]));
        assert_eq!(read_cookie_from(&file, 10), Some(vec![9, 9, 9, 9]));
        assert_eq!(read_cookie_from(&file, 5), None);
    }

    #[test]
    fn ignores_other_authority_names() {
        let file = record("", "0", "XDM-AUTHORIZATION-1", &[7, 7]);
        assert_eq!(read_cookie_from(&file, 0), None);
    }

    #[test]
    fn truncated_files_do_not_panic() {
        let file = record("", "0", "MIT-MAGIC-COOKIE-1", &[1, 2, 3, 4]);
        for length in 0..file.len() {
            let _ = read_cookie_from(&file[..length], 0);
        }
        assert_eq!(read_cookie_from(&file[..6], 0), None);
    }

    #[test]
    fn hex_encoding_matches_openssh() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa1, 0xff]), "000fa1ff");
        assert_eq!(hex(&[]), "");
    }

    #[cfg(unix)]
    #[test]
    fn socket_path_is_the_standard_location() {
        assert_eq!(socket_path(0), PathBuf::from("/tmp/.X11-unix/X0"));
        assert_eq!(socket_path(10), PathBuf::from("/tmp/.X11-unix/X10"));
    }
}
