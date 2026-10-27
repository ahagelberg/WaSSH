//! A blocking MQTT client on its own thread, with a synchronous facade.
//!
//! The analyser is a debug tool, so the subset is deliberately small: one connection, QoS 0
//! subscriptions and publishes, QoS 1 messages acknowledged so brokers stay happy, and a
//! keep-alive ping. Nothing here touches the SSH session; MQTT connects from this machine.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::codec::{self, Packet};

/// Where to connect and who to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MqttOptions {
    /// Broker host name or address.
    pub host: String,
    /// Broker port, usually 1883.
    pub port: u16,
    /// Client id sent in CONNECT.
    pub client_id: String,
    /// Optional user name.
    pub username: Option<String>,
    /// Optional password.
    pub password: Option<String>,
    /// Keep-alive interval in seconds.
    pub keep_alive_s: u16,
    /// Address to dial instead of `host:port`, when the broker is reached
    /// through a tunnel (`127.0.0.1:41234` for an SSH forward).
    pub dial: Option<String>,
}

impl MqttOptions {
    /// Options for a broker with a generated client id.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            client_id: format!("wassh-{}", std::process::id()),
            username: None,
            password: None,
            keep_alive_s: 30,
            dial: None,
        }
    }
}

/// What the analyser hears from the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MqttEvent {
    /// CONNECT was accepted.
    Connected,
    /// A message arrived.
    Message {
        /// Topic it arrived on.
        topic: String,
        /// Raw payload.
        payload: Vec<u8>,
        /// Delivery quality it arrived with.
        qos: u8,
        /// Whether the broker marked it retained.
        retain: bool,
    },
    /// Something went wrong; the connection is gone.
    Error(String),
    /// The connection closed.
    Closed,
}

/// Commands the UI thread sends to the client thread.
enum Command {
    Subscribe(String),
    Unsubscribe(String),
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    },
    Disconnect,
}

/// How long a read waits before the thread looks at its command queue again.
const READ_POLL: Duration = Duration::from_millis(100);

/// A connection to a broker.
pub struct MqttSession {
    commands: Sender<Command>,
    events: Receiver<MqttEvent>,
}

impl MqttSession {
    /// Connect in the background. Progress arrives through [`Self::events`].
    pub fn connect(options: MqttOptions) -> Result<Self, String> {
        let (command_tx, command_rx) = channel();
        let (event_tx, event_rx) = channel();
        std::thread::Builder::new()
            .name("wassh-mqtt".to_owned())
            .spawn(move || run(options, command_rx, event_tx))
            .map_err(|error| format!("starting the MQTT thread: {error}"))?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
        })
    }

    /// Subscribe to `filter` (wildcards allowed).
    pub fn subscribe(&self, filter: &str) {
        let _ = self.commands.send(Command::Subscribe(filter.to_owned()));
    }

    /// Stop receiving `filter`.
    pub fn unsubscribe(&self, filter: &str) {
        let _ = self.commands.send(Command::Unsubscribe(filter.to_owned()));
    }

    /// Publish `payload` to `topic` at QoS 0.
    pub fn publish(&self, topic: &str, payload: &[u8]) {
        self.publish_qos(topic, payload, 0, false);
    }

    /// Publish with an explicit QoS (0 or 1) and retain flag.
    pub fn publish_qos(&self, topic: &str, payload: &[u8], qos: u8, retain: bool) {
        let _ = self.commands.send(Command::Publish {
            topic: topic.to_owned(),
            payload: payload.to_vec(),
            qos: qos.min(1),
            retain,
        });
    }

    /// Close the connection.
    pub fn disconnect(&self) {
        let _ = self.commands.send(Command::Disconnect);
    }

    /// Drain the events that have arrived.
    pub fn events(&self) -> Vec<MqttEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for MqttSession {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Disconnect);
    }
}

/// The client thread.
fn run(options: MqttOptions, commands: Receiver<Command>, events: Sender<MqttEvent>) {
    // A tunnelled broker is dialled on loopback; the host in the CONNECT packet
    // is what the broker sees, not what we connect to.
    let dial = options
        .dial
        .clone()
        .unwrap_or_else(|| format!("{}:{}", options.host, options.port));
    let mut stream = match TcpStream::connect(dial.as_str()) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = events.send(MqttEvent::Error(format!(
                "connecting to {}:{}: {error}",
                options.host, options.port
            )));
            return;
        }
    };
    let _ = stream.set_read_timeout(Some(READ_POLL));
    let _ = stream.set_nodelay(true);

    let hello = codec::encode_connect(
        &options.client_id,
        options.keep_alive_s,
        options.username.as_deref(),
        options.password.as_deref(),
    );
    if let Err(error) = stream.write_all(&hello) {
        let _ = events.send(MqttEvent::Error(format!("sending CONNECT: {error}")));
        return;
    }

    let keep_alive = Duration::from_secs(u64::from(options.keep_alive_s.max(5)));
    let mut last_sent = Instant::now();
    let mut next_id: u16 = 1;
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];

    loop {
        // Commands first, so a publish feels immediate.
        loop {
            match commands.try_recv() {
                Ok(Command::Subscribe(filter)) => {
                    let packet = codec::encode_subscribe(next_id, &filter);
                    next_id = next_id.wrapping_add(1).max(1);
                    if stream.write_all(&packet).is_err() {
                        let _ = events.send(MqttEvent::Error("sending SUBSCRIBE failed".to_owned()));
                        let _ = events.send(MqttEvent::Closed);
                        return;
                    }
                    last_sent = Instant::now();
                }
                Ok(Command::Unsubscribe(filter)) => {
                    let packet = codec::encode_unsubscribe(next_id, &filter);
                    next_id = next_id.wrapping_add(1).max(1);
                    let _ = stream.write_all(&packet);
                    last_sent = Instant::now();
                }
                Ok(Command::Publish {
                    topic,
                    payload,
                    qos,
                    retain,
                }) => {
                    let packet = codec::encode_publish_with(
                        &topic,
                        &payload,
                        qos,
                        retain,
                        next_id,
                    );
                    if qos > 0 {
                        next_id = next_id.wrapping_add(1).max(1);
                    }
                    if let Err(error) = stream.write_all(&packet) {
                        let _ = events.send(MqttEvent::Error(format!("publishing: {error}")));
                    }
                    last_sent = Instant::now();
                }
                Ok(Command::Disconnect) => {
                    let _ = stream.write_all(&codec::encode_disconnect());
                    let _ = events.send(MqttEvent::Closed);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let _ = stream.write_all(&codec::encode_disconnect());
                    let _ = events.send(MqttEvent::Closed);
                    return;
                }
            }
        }

        // Then whatever the broker sent.
        match stream.read(&mut chunk) {
            Ok(0) => {
                let _ = events.send(MqttEvent::Closed);
                return;
            }
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                let mut consumed = 0usize;
                loop {
                    let decoded = codec::decode(&buffer[consumed..]);
                    match decoded {
                        Ok(Some((packet, used))) => {
                            consumed += used;
                            if !handle(packet, &mut stream, &events) {
                                let _ = events.send(MqttEvent::Closed);
                                return;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            let _ = events.send(MqttEvent::Error(format!("bad packet: {error}")));
                            let _ = events.send(MqttEvent::Closed);
                            return;
                        }
                    }
                }
                buffer.drain(..consumed);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                let _ = events.send(MqttEvent::Error(format!("reading from the broker: {error}")));
                let _ = events.send(MqttEvent::Closed);
                return;
            }
        }

        if last_sent.elapsed() >= keep_alive / 2 {
            if stream.write_all(&codec::encode_pingreq()).is_err() {
                let _ = events.send(MqttEvent::Error("keep-alive failed".to_owned()));
                let _ = events.send(MqttEvent::Closed);
                return;
            }
            last_sent = Instant::now();
        }
    }
}

/// Handle one decoded packet. Returns false when the connection should end.
fn handle(packet: Packet, stream: &mut TcpStream, events: &Sender<MqttEvent>) -> bool {
    match packet {
        Packet::ConnAck { code, .. } => {
            if code != 0 {
                let _ = events.send(MqttEvent::Error(format!(
                    "broker refused the connection: {}",
                    Packet::refusal_reason(code)
                )));
                return false;
            }
            let _ = events.send(MqttEvent::Connected);
        }
        Packet::Publish {
            topic,
            payload,
            qos,
            retain,
            packet_id,
        } => {
            if qos == 1 {
                // Acknowledge so the broker stops retrying.
                let _ = stream.write_all(&codec::encode_puback(packet_id));
            }
            let _ = events.send(MqttEvent::Message {
                topic,
                payload,
                qos,
                retain,
            });
        }
        Packet::SubAck { .. } | Packet::PingResp | Packet::Other(_) => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Buffers events so a test can read them one at a time; `events()` drains everything.
    struct Events<'a> {
        session: &'a MqttSession,
        queue: std::collections::VecDeque<MqttEvent>,
    }

    impl<'a> Events<'a> {
        fn new(session: &'a MqttSession) -> Self {
            Self {
                session,
                queue: std::collections::VecDeque::new(),
            }
        }

        /// Wait for the next event, or give up.
        fn next(&mut self) -> MqttEvent {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(event) = self.queue.pop_front() {
                    return event;
                }
                if Instant::now() > deadline {
                    panic!("no MQTT event within five seconds");
                }
                self.queue.extend(self.session.events());
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    #[test]
    fn a_broker_that_accepts_and_publishes_is_understood() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();

        // A broker that accepts CONNECT, acknowledges it, then sends one message.
        let broker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 1024];
            let read = stream.read(&mut buffer).expect("read CONNECT");
            assert_eq!(buffer[0], 0x10, "first packet is CONNECT");
            assert!(read > 2);
            stream.write_all(&[0x20, 0x02, 0x00, 0x00]).expect("CONNACK");
            stream
                .write_all(&codec::encode_publish("demo/topic", b"hello broker"))
                .expect("publish");
            std::thread::sleep(Duration::from_millis(500));
        });

        let session = MqttSession::connect(MqttOptions::new("127.0.0.1", port)).expect("connect");
        let mut events = Events::new(&session);
        assert_eq!(events.next(), MqttEvent::Connected);
        assert_eq!(
            events.next(),
            MqttEvent::Message {
                topic: "demo/topic".to_owned(),
                payload: b"hello broker".to_vec(),
                qos: 0,
                retain: false,
            }
        );
        drop(session);
        let _ = broker.join();
    }

    #[test]
    fn a_refusal_ends_the_connection_with_a_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let broker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            // Refusal code 5: not authorised.
            stream.write_all(&[0x20, 0x02, 0x00, 0x05]).expect("CONNACK");
            std::thread::sleep(Duration::from_millis(200));
        });

        let session = MqttSession::connect(MqttOptions::new("127.0.0.1", port)).expect("connect");
        let mut events = Events::new(&session);
        match events.next() {
            MqttEvent::Error(message) => {
                assert!(message.contains("not authorised"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        let _ = broker.join();
    }

    #[test]
    fn an_unreachable_broker_reports_an_error() {
        // Port 1 on loopback has nothing listening.
        let session = MqttSession::connect(MqttOptions::new("127.0.0.1", 1)).expect("spawn");
        let mut events = Events::new(&session);
        match events.next() {
            MqttEvent::Error(message) => assert!(message.contains("connecting"), "{message}"),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn subscribing_sends_the_filter_to_the_broker() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let broker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            stream.write_all(&[0x20, 0x02, 0x00, 0x00]).expect("CONNACK");
            // Read the SUBSCRIBE that follows.
            let read = stream.read(&mut buffer).expect("read SUBSCRIBE");
            let packet = &buffer[..read];
            assert_eq!(packet[0], 0x82, "SUBSCRIBE header");
            assert!(
                packet.windows(7).any(|window| window == b"sensors"),
                "filter was not sent"
            );
            std::thread::sleep(Duration::from_millis(200));
        });

        let session = MqttSession::connect(MqttOptions::new("127.0.0.1", port)).expect("connect");
        let mut events = Events::new(&session);
        assert_eq!(events.next(), MqttEvent::Connected);
        session.subscribe("sensors/#");
        std::thread::sleep(Duration::from_millis(300));
        drop(session);
        let _ = broker.join();
    }
}
