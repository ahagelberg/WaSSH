//! Telnet transport.
//!
//! A synchronous facade over a tokio TCP connection, matching `wassh_ssh::SshSession`:
//! commands go in through an unbounded channel, data and status events come out through a
//! standard `mpsc` receiver. All IAC negotiation happens on the worker thread.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use wassh_core::{DEFAULT_TERM_TYPE, SessionStatus};

use crate::codec::TelnetFilter;

/// How to reach the server.
#[derive(Clone, Debug)]
pub struct TelnetOptions {
    pub host: String,
    pub port: u16,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    pub connect_timeout: Duration,
}

impl TelnetOptions {
    /// Options for `host:port` with a default 80×24 window.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            term: DEFAULT_TERM_TYPE.to_owned(),
            cols: 80,
            rows: 24,
            connect_timeout: Duration::from_secs(20),
        }
    }
}

/// Events reported to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelnetEvent {
    Data(Vec<u8>),
    Status(SessionStatus),
    Closed {
        reason: Option<String>,
    },
    Error(String),
}

enum TelnetCommand {
    Write(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Disconnect,
}

/// Synchronous handle to a background Telnet connection.
pub struct TelnetSession {
    commands: UnboundedSender<TelnetCommand>,
    events: Receiver<TelnetEvent>,
    size: (u16, u16),
}

impl TelnetSession {
    /// Connect in the background.
    pub fn connect(options: TelnetOptions) -> Result<Self> {
        let (command_tx, command_rx) = unbounded_channel();
        let (event_tx, event_rx) = channel();
        let size = (options.cols, options.rows);

        std::thread::Builder::new()
            .name("wassh-telnet".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_tx.send(TelnetEvent::Error(format!(
                            "cannot start the async runtime: {error}"
                        )));
                        return;
                    }
                };
                runtime.block_on(run(options, command_rx, event_tx));
            })
            .context("starting the Telnet thread")?;

        Ok(Self {
            commands: command_tx,
            events: event_rx,
            size,
        })
    }

    /// Send application bytes (they are IAC-escaped here).
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.commands.send(TelnetCommand::Write(bytes.to_vec()));
    }

    /// Update the window size; NAWS is sent when the server negotiated it.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) != self.size {
            self.size = (cols, rows);
            let _ = self.commands.send(TelnetCommand::Resize { cols, rows });
        }
    }

    /// Close the connection.
    pub fn disconnect(&mut self) {
        let _ = self.commands.send(TelnetCommand::Disconnect);
    }

    /// Drain pending events without blocking.
    pub fn events(&self) -> Vec<TelnetEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for TelnetSession {
    fn drop(&mut self) {
        let _ = self.commands.send(TelnetCommand::Disconnect);
    }
}

async fn run(
    options: TelnetOptions,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<TelnetCommand>,
    events: std::sync::mpsc::Sender<TelnetEvent>,
) {
    let status = |status| {
        let _ = events.send(TelnetEvent::Status(status));
    };
    status(SessionStatus::Connecting);

    let stream = match tokio::time::timeout(
        options.connect_timeout,
        TcpStream::connect((options.host.as_str(), options.port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            let _ = events.send(TelnetEvent::Error(format!(
                "cannot connect to {}:{}: {error}",
                options.host, options.port
            )));
            let _ = events.send(TelnetEvent::Closed { reason: None });
            return;
        }
        Err(_) => {
            let _ = events.send(TelnetEvent::Error(anyhow!(
                "connection to {}:{} timed out",
                options.host,
                options.port
            )
            .to_string()));
            let _ = events.send(TelnetEvent::Closed { reason: None });
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();

    let mut filter = TelnetFilter::new(options.term.clone(), options.cols, options.rows);
    let greeting = filter.start();
    if writer.write_all(&greeting).await.is_err() {
        let _ = events.send(TelnetEvent::Closed { reason: None });
        return;
    }

    status(SessionStatus::Connected);

    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(TelnetCommand::Write(bytes)) => {
                        let escaped = TelnetFilter::escape(&bytes);
                        if writer.write_all(&escaped).await.is_err() {
                            break;
                        }
                    }
                    Some(TelnetCommand::Resize { cols, rows }) => {
                        if let Some(payload) = filter.resize(cols, rows)
                            && writer.write_all(&payload).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(TelnetCommand::Disconnect) | None => break,
                }
            }
            read = reader.read(&mut buffer) => {
                match read {
                    Ok(0) => break,
                    Ok(count) => {
                        let data = filter.feed(&buffer[..count]);
                        let replies = filter.take_replies();
                        if !replies.is_empty() && writer.write_all(&replies).await.is_err() {
                            break;
                        }
                        if !data.is_empty() {
                            let _ = events.send(TelnetEvent::Data(data));
                        }
                    }
                    Err(error) => {
                        let _ = events.send(TelnetEvent::Error(error.to_string()));
                        break;
                    }
                }
            }
        }
    }

    let _ = writer.shutdown().await;
    let _ = events.send(TelnetEvent::Closed { reason: None });
}
