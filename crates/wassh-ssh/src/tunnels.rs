//! SSH port forwarding: local (`-L`), remote (`-R`) and dynamic SOCKS5 (`-D`).
//!
//! Local and dynamic forwards bind a loopback listener and open a `direct-tcpip` channel
//! per accepted connection. Remote forwards ask the server to listen and answer each
//! `forwarded-tcpip` channel by dialling the destination from this machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use russh::ChannelOpenFailure;
use russh::client::Handle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wassh_core::{Tunnel, TunnelKind};

/// A forward to establish when a session connects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelRequest {
    /// `-L`, `-R` or `-D`.
    pub kind: TunnelKind,
    /// Address the listener binds to (local for `-L`/`-D`, remote for `-R`).
    pub listen_host: String,
    /// Port the listener binds to; 0 lets the OS pick (local forwards only).
    pub listen_port: u16,
    /// Destination host; unused for `-D`.
    pub dest_host: String,
    /// Destination port; unused for `-D`.
    pub dest_port: u16,
}

impl TunnelRequest {
    /// Convert a saved tunnel, returning `None` for disabled or incomplete entries.
    pub fn from_saved(tunnel: &Tunnel) -> Option<Self> {
        if !tunnel.enabled {
            return None;
        }
        if tunnel.kind != TunnelKind::Dynamic
            && (tunnel.dest_host.trim().is_empty() || tunnel.dest_port == 0)
        {
            return None;
        }
        Some(Self {
            kind: tunnel.kind,
            listen_host: if tunnel.listen_host.trim().is_empty() {
                "127.0.0.1".to_owned()
            } else {
                tunnel.listen_host.clone()
            },
            listen_port: tunnel.listen_port,
            dest_host: tunnel.dest_host.clone(),
            dest_port: tunnel.dest_port,
        })
    }

    /// Human summary used in status messages.
    pub fn summary(&self) -> String {
        match self.kind {
            TunnelKind::Local => format!(
                "L {}:{} → {}:{}",
                self.listen_host, self.listen_port, self.dest_host, self.dest_port
            ),
            TunnelKind::Remote => format!(
                "R {}:{} → {}:{}",
                self.listen_host, self.listen_port, self.dest_host, self.dest_port
            ),
            TunnelKind::Dynamic => {
                format!("SOCKS5 {}:{}", self.listen_host, self.listen_port)
            }
        }
    }
}

/// Server-initiated `forwarded-tcpip` channels, keyed by the bound address, valued by the
/// destination to dial locally.
pub type RemoteForwards = Arc<Mutex<HashMap<(String, u32), (String, u16)>>>;

/// An empty forward table.
pub fn remote_forwards() -> RemoteForwards {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Look up where a server-initiated remote forward should be dialled.
pub fn resolve_remote(
    forwards: &RemoteForwards,
    address: &str,
    port: u32,
) -> Option<(String, u16)> {
    forwards
        .lock()
        .expect("remote forward table")
        .get(&(address.to_owned(), port))
        .cloned()
}

/// Establish one forward. Returns a description for the status strip.
pub async fn start<H>(
    handle: Arc<Handle<H>>,
    request: &TunnelRequest,
    forwards: &RemoteForwards,
) -> Result<String>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    match request.kind {
        TunnelKind::Local => start_local(handle, request).await,
        TunnelKind::Dynamic => start_dynamic(handle, request).await,
        TunnelKind::Remote => start_remote(handle, request, forwards).await,
    }
}

async fn start_local<H>(handle: Arc<Handle<H>>, request: &TunnelRequest) -> Result<String>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    let (host, port) = start_local_forward(handle, &request.listen_host, request.listen_port, &request.dest_host, request.dest_port)
        .await?;
    Ok(format!(
        "L {host}:{port} → {}:{}",
        request.dest_host, request.dest_port
    ))
}

/// Bind a loopback listener that forwards every connection to `dest` over the
/// session, and return the address it listens on.
///
/// Used both by saved `-L` tunnels and by panes that need a plain TCP client to
/// reach a service on the remote host (the MQTT analyser's broker).
pub async fn start_local_forward<H>(
    handle: Arc<Handle<H>>,
    listen_host: &str,
    listen_port: u16,
    dest_host: &str,
    dest_port: u16,
) -> Result<(String, u16)>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    let listener = TcpListener::bind((listen_host, listen_port))
        .await
        .with_context(|| format!("binding the local listener {listen_host}:{listen_port}"))?;
    let bound = listener
        .local_addr()
        .context("reading the listener address")?;
    let dest = (dest_host.to_owned(), u32::from(dest_port));

    tokio::spawn(async move {
        loop {
            let Ok((socket, peer)) = listener.accept().await else {
                break;
            };
            let handle = handle.clone();
            let dest = dest.clone();
            tokio::spawn(async move {
                let channel = match handle
                    .channel_open_direct_tcpip(
                        dest.0.clone(),
                        dest.1,
                        peer.ip().to_string(),
                        u32::from(peer.port()),
                    )
                    .await
                {
                    Ok(channel) => channel,
                    Err(_) => return,
                };
                let mut remote = channel.into_stream();
                let mut socket = socket;
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut remote).await;
            });
        }
    });

    Ok((bound.ip().to_string(), bound.port()))
}

async fn start_dynamic<H>(handle: Arc<Handle<H>>, request: &TunnelRequest) -> Result<String>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    let listener = TcpListener::bind((request.listen_host.as_str(), request.listen_port))
        .await
        .with_context(|| {
            format!(
                "binding the SOCKS listener {}:{}",
                request.listen_host, request.listen_port
            )
        })?;
    let bound = listener
        .local_addr()
        .context("reading the listener address")?;

    tokio::spawn(async move {
        loop {
            let Ok((socket, peer)) = listener.accept().await else {
                break;
            };
            let handle = handle.clone();
            tokio::spawn(async move {
                let _ = serve_socks5(socket, handle, peer).await;
            });
        }
    });

    Ok(format!("SOCKS5 {}:{}", bound.ip(), bound.port()))
}

async fn start_remote<H>(
    handle: Arc<Handle<H>>,
    request: &TunnelRequest,
    forwards: &RemoteForwards,
) -> Result<String>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    let address = if request.listen_host.trim().is_empty() {
        "127.0.0.1".to_owned()
    } else {
        request.listen_host.clone()
    };
    let assigned = handle
        .tcpip_forward(address.clone(), u32::from(request.listen_port))
        .await
        .context("requesting a remote forward")?;
    forwards.lock().expect("remote forward table").insert(
        (address.clone(), assigned),
        (request.dest_host.clone(), request.dest_port),
    );
    Ok(format!(
        "R {}:{} → {}:{}",
        address, assigned, request.dest_host, request.dest_port
    ))
}

/// Handle one SOCKS5 client: greeting, CONNECT, then pump bytes.
async fn serve_socks5<H>(
    mut socket: TcpStream,
    handle: Arc<Handle<H>>,
    peer: std::net::SocketAddr,
) -> Result<()>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    // Greeting: version, method count, methods. We only support "no authentication".
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await?;
    if head[0] != SOCKS5_VERSION {
        return Err(anyhow!("unsupported SOCKS version {}", head[0]));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    socket.read_exact(&mut methods).await?;
    socket
        .write_all(&[SOCKS5_VERSION, SOCKS5_NO_AUTH])
        .await?;

    let mut header = [0u8; 4];
    socket.read_exact(&mut header).await?;
    if header[0] != SOCKS5_VERSION || header[1] != SOCKS5_CONNECT {
        socket
            .write_all(&[SOCKS5_VERSION, SOCKS5_UNSUPPORTED_COMMAND, 0, 1, 0, 0, 0, 0, 0, 0])
            .await?;
        return Ok(());
    }
    let mut tail = Vec::new();
    match header[3] {
        SOCKS5_ATYP_IPV4 => {
            tail.resize(4 + 2, 0);
            socket.read_exact(&mut tail).await?;
        }
        SOCKS5_ATYP_DOMAIN => {
            let mut length = [0u8; 1];
            socket.read_exact(&mut length).await?;
            tail.resize(usize::from(length[0]) + 2, 0);
            socket.read_exact(&mut tail).await?;
        }
        SOCKS5_ATYP_IPV6 => {
            tail.resize(16 + 2, 0);
            socket.read_exact(&mut tail).await?;
        }
        _ => {
            socket
                .write_all(&[SOCKS5_VERSION, SOCKS5_UNSUPPORTED_ATYP, 0, 1, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(());
        }
    }
    let (host, port) = parse_socks_address(&[header[3]], &tail)
        .ok_or_else(|| anyhow!("malformed SOCKS5 request"))?;

    let channel = match handle
        .channel_open_direct_tcpip(
            host.clone(),
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await
    {
        Ok(channel) => channel,
        Err(_) => {
            socket
                .write_all(&[SOCKS5_VERSION, SOCKS5_HOST_UNREACHABLE, 0, 1, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(());
        }
    };

    socket
        .write_all(&[SOCKS5_VERSION, SOCKS5_SUCCEEDED, 0, 1, 0, 0, 0, 0, 0, 0])
        .await?;
    let mut remote = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut remote).await;
    Ok(())
}

pub const SOCKS5_VERSION: u8 = 5;
pub const SOCKS5_NO_AUTH: u8 = 0;
pub const SOCKS5_SUCCEEDED: u8 = 0;
pub const SOCKS5_CONNECT: u8 = 1;
pub const SOCKS5_UNSUPPORTED_COMMAND: u8 = 7;
pub const SOCKS5_UNSUPPORTED_ATYP: u8 = 8;
pub const SOCKS5_HOST_UNREACHABLE: u8 = 4;
pub const SOCKS5_ATYP_IPV4: u8 = 1;
pub const SOCKS5_ATYP_DOMAIN: u8 = 3;
pub const SOCKS5_ATYP_IPV6: u8 = 4;

/// Decode a SOCKS5 destination from its address-type byte plus the bytes that follow.
pub fn parse_socks_address(atyp: &[u8], rest: &[u8]) -> Option<(String, u16)> {
    let atyp = *atyp.first()?;
    match atyp {
        SOCKS5_ATYP_IPV4 => {
            if rest.len() < 6 {
                return None;
            }
            let host = format!("{}.{}.{}.{}", rest[0], rest[1], rest[2], rest[3]);
            let port = u16::from_be_bytes([rest[4], rest[5]]);
            Some((host, port))
        }
        SOCKS5_ATYP_DOMAIN => {
            let length = usize::from(*rest.first()?);
            if rest.len() < 1 + length + 2 {
                return None;
            }
            let host = String::from_utf8(rest[1..1 + length].to_vec()).ok()?;
            let port = u16::from_be_bytes([rest[1 + length], rest[2 + length]]);
            Some((host, port))
        }
        SOCKS5_ATYP_IPV6 => {
            if rest.len() < 18 {
                return None;
            }
            let octets: Vec<String> = rest[..16].chunks(2).map(|chunk| {
                format!("{:02x}{:02x}", chunk[0], chunk[1])
            }).collect();
            let host = octets.join(":");
            let port = u16::from_be_bytes([rest[16], rest[17]]);
            Some((host, port))
        }
        _ => None,
    }
}

/// Ask the server to stop listening for a remote forward.
pub async fn stop_remote<H>(handle: &Handle<H>, address: &str, port: u32) -> Result<()>
where
    H: russh::client::Handler + Send + Sync + 'static,
{
    handle
        .cancel_tcpip_forward(address.to_owned(), port)
        .await
        .context("cancelling the remote forward")
}

/// Reasons a forwarded channel may be refused.
pub const REJECT_FORWARD: ChannelOpenFailure = ChannelOpenFailure::AdministrativelyProhibited;

#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::Tunnel;

    fn saved(kind: TunnelKind, enabled: bool) -> Tunnel {
        Tunnel {
            id: "t".to_owned(),
            enabled,
            kind,
            listen_host: "127.0.0.1".to_owned(),
            listen_port: 8080,
            dest_host: "db.internal".to_owned(),
            dest_port: 5432,
        }
    }

    #[test]
    fn disabled_tunnels_are_skipped() {
        assert!(TunnelRequest::from_saved(&saved(TunnelKind::Local, false)).is_none());
    }

    #[test]
    fn incomplete_local_tunnels_are_skipped() {
        let mut tunnel = saved(TunnelKind::Local, true);
        tunnel.dest_host.clear();
        assert!(TunnelRequest::from_saved(&tunnel).is_none());
        tunnel.dest_host = "db.internal".to_owned();
        tunnel.dest_port = 0;
        assert!(TunnelRequest::from_saved(&tunnel).is_none());
        tunnel.dest_port = 5432;
        assert!(TunnelRequest::from_saved(&tunnel).is_some());
    }

    #[test]
    fn dynamic_tunnels_need_no_destination() {
        let mut tunnel = saved(TunnelKind::Dynamic, true);
        tunnel.dest_host.clear();
        tunnel.dest_port = 0;
        let request = TunnelRequest::from_saved(&tunnel).expect("valid");
        assert_eq!(request.summary(), "SOCKS5 127.0.0.1:8080");
    }

    #[test]
    fn summaries_match_the_core_model() {
        assert_eq!(
            TunnelRequest::from_saved(&saved(TunnelKind::Local, true))
                .unwrap()
                .summary(),
            "L 127.0.0.1:8080 → db.internal:5432"
        );
        assert_eq!(
            TunnelRequest::from_saved(&saved(TunnelKind::Remote, true))
                .unwrap()
                .summary(),
            "R 127.0.0.1:8080 → db.internal:5432"
        );
    }

    #[test]
    fn socks5_ipv4_destination_parses() {
        let rest = [127, 0, 0, 1, 0x1f, 0x90];
        assert_eq!(
            parse_socks_address(&[SOCKS5_ATYP_IPV4], &rest),
            Some(("127.0.0.1".to_owned(), 8080))
        );
    }

    #[test]
    fn socks5_domain_destination_parses() {
        let mut rest = vec![9];
        rest.extend_from_slice(b"localhost");
        rest.extend_from_slice(&[0x00, 0x50]);
        assert_eq!(
            parse_socks_address(&[SOCKS5_ATYP_DOMAIN], &rest),
            Some(("localhost".to_owned(), 80))
        );
    }

    #[test]
    fn socks5_ipv6_destination_parses() {
        let mut rest = vec![0u8; 18];
        rest[15] = 1;
        rest[16] = 0;
        rest[17] = 22;
        let (host, port) = parse_socks_address(&[SOCKS5_ATYP_IPV6], &rest).unwrap();
        assert_eq!(host, "0000:0000:0000:0000:0000:0000:0000:0001");
        assert_eq!(port, 22);
    }

    #[test]
    fn truncated_requests_are_rejected() {
        assert!(parse_socks_address(&[SOCKS5_ATYP_IPV4], &[1, 2]).is_none());
        assert!(parse_socks_address(&[SOCKS5_ATYP_DOMAIN], &[9, b'a']).is_none());
        assert!(parse_socks_address(&[99], &[]).is_none());
    }

    #[test]
    fn remote_forward_table_resolves() {
        let table = remote_forwards();
        table
            .lock()
            .unwrap()
            .insert(("127.0.0.1".to_owned(), 2200), ("localhost".to_owned(), 22));
        assert_eq!(
            resolve_remote(&table, "127.0.0.1", 2200),
            Some(("localhost".to_owned(), 22))
        );
        assert_eq!(resolve_remote(&table, "127.0.0.1", 2201), None);
    }
}
