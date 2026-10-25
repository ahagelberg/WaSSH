//! The MQTT 3.1.1 packets the analyser needs.
//!
//! Only the subset a viewer uses is implemented: connect, subscribe, unsubscribe, publish at
//! QoS 0, the acknowledgements that come back, and the keep-alive ping. Everything is a plain
//! byte slice, so it can be tested without a broker.

/// A packet the client understands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    /// The broker answered our CONNECT.
    ConnAck {
        /// Whether the broker resumed an existing session.
        session_present: bool,
        /// 0 means accepted; anything else is a refusal code.
        code: u8,
    },
    /// A message arrived.
    Publish {
        /// Topic it was published to.
        topic: String,
        /// Raw payload.
        payload: Vec<u8>,
        /// Delivery quality it arrived with.
        qos: u8,
        /// Whether the broker keeps it as the topic's retained value.
        retain: bool,
        /// Packet id, present for QoS 1.
        packet_id: u16,
    },
    /// Our SUBSCRIBE was accepted.
    SubAck {
        /// Packet id that was acknowledged.
        packet_id: u16,
    },
    /// Answer to a keep-alive ping.
    PingResp,
    /// Any packet the analyser does not look at.
    Other(u8),
}

impl Packet {
    /// The reason a CONNACK code stands for.
    pub fn refusal_reason(code: u8) -> &'static str {
        match code {
            1 => "unacceptable protocol version",
            2 => "client id rejected",
            3 => "server unavailable",
            4 => "bad username or password",
            5 => "not authorised",
            _ => "unknown reason",
        }
    }
}

/// Append a UTF-8 string with its two-byte length prefix.
fn put_string(buffer: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    // MQTT strings are length-prefixed with 16 bits; truncate rather than emit a bad packet.
    let length = bytes.len().min(u16::MAX as usize);
    buffer.extend_from_slice(&(length as u16).to_be_bytes());
    buffer.extend_from_slice(&bytes[..length]);
}

/// Encode the MQTT remaining-length field.
pub fn encode_remaining_length(mut length: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (length % 128) as u8;
        length /= 128;
        if length > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if length == 0 {
            break;
        }
    }
    out
}

/// Read the MQTT remaining-length field, returning the value and how many bytes it used.
pub fn decode_remaining_length(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    let mut multiplier = 1usize;
    for (index, byte) in bytes.iter().enumerate().take(4) {
        value += (*byte & 0x7F) as usize * multiplier;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
        multiplier *= 128;
    }
    None
}

/// Wrap `body` in a fixed header.
fn frame(header: u8, body: Vec<u8>) -> Vec<u8> {
    let mut packet = Vec::with_capacity(body.len() + 5);
    packet.push(header);
    packet.extend_from_slice(&encode_remaining_length(body.len()));
    packet.extend_from_slice(&body);
    packet
}

/// Protocol name and level for MQTT 3.1.1.
const PROTOCOL_NAME: &str = "MQTT";
/// Protocol level byte for MQTT 3.1.1.
const PROTOCOL_LEVEL: u8 = 4;

/// Build the CONNECT packet for `options`.
pub fn encode_connect(
    host_client_id: &str,
    keep_alive_s: u16,
    username: Option<&str>,
    password: Option<&str>,
) -> Vec<u8> {
    let mut flags = 0x02; // Clean session.
    if username.is_some() {
        flags |= 0x80;
    }
    if password.is_some() {
        flags |= 0x40;
    }
    let mut body = Vec::new();
    put_string(&mut body, PROTOCOL_NAME);
    body.push(PROTOCOL_LEVEL);
    body.push(flags);
    body.extend_from_slice(&keep_alive_s.to_be_bytes());
    put_string(&mut body, host_client_id);
    if let Some(username) = username {
        put_string(&mut body, username);
    }
    if let Some(password) = password {
        put_string(&mut body, password);
    }
    frame(0x10, body)
}

/// Build a SUBSCRIBE for `filter` at QoS 0.
pub fn encode_subscribe(packet_id: u16, filter: &str) -> Vec<u8> {
    let mut body = packet_id.to_be_bytes().to_vec();
    put_string(&mut body, filter);
    body.push(0x00);
    frame(0x82, body)
}

/// Build an UNSUBSCRIBE for `filter`.
pub fn encode_unsubscribe(packet_id: u16, filter: &str) -> Vec<u8> {
    let mut body = packet_id.to_be_bytes().to_vec();
    put_string(&mut body, filter);
    frame(0xA2, body)
}

/// Build a PUBLISH at QoS 0.
pub fn encode_publish(topic: &str, payload: &[u8]) -> Vec<u8> {
    encode_publish_with(topic, payload, 0, false, 0)
}

/// Build a PUBLISH with the QoS and retain flag the user chose.
pub fn encode_publish_with(
    topic: &str,
    payload: &[u8],
    qos: u8,
    retain: bool,
    packet_id: u16,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(topic.len() + payload.len() + 4);
    put_string(&mut body, topic);
    if qos > 0 {
        body.extend_from_slice(&packet_id.to_be_bytes());
    }
    body.extend_from_slice(payload);
    let header = 0x30 | ((qos & 0x03) << 1) | u8::from(retain);
    frame(header, body)
}

/// Build the PUBACK for a QoS 1 message we accepted.
pub fn encode_puback(packet_id: u16) -> Vec<u8> {
    frame(0x40, packet_id.to_be_bytes().to_vec())
}

/// Build a keep-alive ping.
pub fn encode_pingreq() -> Vec<u8> {
    vec![0xC0, 0x00]
}

/// Build the polite DISCONNECT sent when the analyser stops.
pub fn encode_disconnect() -> Vec<u8> {
    vec![0xE0, 0x00]
}

/// Read a length-prefixed string, returning it and the rest of the buffer.
fn read_string(bytes: &[u8]) -> Result<(String, &[u8]), String> {
    if bytes.len() < 2 {
        return Err("truncated string length".to_owned());
    }
    let length = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    let end = 2 + length;
    if bytes.len() < end {
        return Err("truncated string".to_owned());
    }
    let text = String::from_utf8_lossy(&bytes[2..end]).into_owned();
    Ok((text, &bytes[end..]))
}

/// Decode one packet from `buffer`.
///
/// Returns `Ok(None)` when the buffer does not hold a whole packet yet, so a reader can simply
/// wait for more bytes.
pub fn decode(buffer: &[u8]) -> Result<Option<(Packet, usize)>, String> {
    if buffer.len() < 2 {
        return Ok(None);
    }
    let Some((length, length_bytes)) = decode_remaining_length(&buffer[1..]) else {
        return Ok(None);
    };
    let header_len = 1 + length_bytes;
    if buffer.len() < header_len + length {
        return Ok(None);
    }
    let body = &buffer[header_len..header_len + length];
    let kind = buffer[0] >> 4;
    let flags = buffer[0] & 0x0F;
    let packet = match kind {
        2 => {
            if body.len() < 2 {
                return Err("truncated CONNACK".to_owned());
            }
            Packet::ConnAck {
                session_present: body[0] & 0x01 == 1,
                code: body[1],
            }
        }
        3 => {
            let qos = (flags >> 1) & 0x03;
            let retain = flags & 0x01 == 1;
            let (topic, rest) = read_string(body)?;
            let (packet_id, payload) = if qos > 0 {
                if rest.len() < 2 {
                    return Err("truncated publish id".to_owned());
                }
                (
                    u16::from_be_bytes([rest[0], rest[1]]),
                    rest[2..].to_vec(),
                )
            } else {
                (0, rest.to_vec())
            };
            Packet::Publish {
                topic,
                payload,
                qos,
                retain,
                packet_id,
            }
        }
        4 => {
            if body.len() < 2 {
                return Err("truncated PUBACK".to_owned());
            }
            Packet::Other(4)
        }
        9 => {
            if body.len() < 2 {
                return Err("truncated SUBACK".to_owned());
            }
            Packet::SubAck {
                packet_id: u16::from_be_bytes([body[0], body[1]]),
            }
        }
        13 => Packet::PingResp,
        other => Packet::Other(other),
    };
    Ok(Some((packet, header_len + length)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_lengths_cover_every_size() {
        for (length, bytes) in [
            (0usize, vec![0x00]),
            (127, vec![0x7F]),
            (128, vec![0x80, 0x01]),
            (16_383, vec![0xFF, 0x7F]),
            (16_384, vec![0x80, 0x80, 0x01]),
        ] {
            assert_eq!(encode_remaining_length(length), bytes);
            assert_eq!(decode_remaining_length(&bytes), Some((length, bytes.len())));
        }
        // An incomplete varint says "need more bytes".
        assert_eq!(decode_remaining_length(&[0x80]), None);
    }

    #[test]
    fn connect_names_the_protocol_and_credentials() {
        let packet = encode_connect("wassh-1", 60, Some("alice"), Some("secret"));
        assert_eq!(packet[0], 0x10);
        let (length, used) = decode_remaining_length(&packet[1..]).expect("length");
        assert_eq!(packet.len(), 1 + used + length);

        let body = &packet[1 + used..];
        // "MQTT" with its length prefix, then level 4.
        assert_eq!(&body[..6], b"\x00\x04MQTT");
        assert_eq!(body[6], 4);
        // Clean session plus username and password flags.
        assert_eq!(body[7], 0x02 | 0x80 | 0x40);
        assert_eq!(&body[8..10], &60u16.to_be_bytes());
        assert!(packet.windows(5).any(|window| window == b"alice"));
        assert!(packet.windows(6).any(|window| window == b"secret"));
    }

    #[test]
    fn anonymous_connect_sets_no_credential_flags() {
        let packet = encode_connect("wassh-1", 30, None, None);
        let (_, used) = decode_remaining_length(&packet[1..]).expect("length");
        assert_eq!(packet[1 + used + 7], 0x02);
    }

    #[test]
    fn subscribe_asks_for_qos_zero() {
        let packet = encode_subscribe(7, "sensors/#");
        assert_eq!(packet[0], 0x82);
        let (_, used) = decode_remaining_length(&packet[1..]).expect("length");
        let body = &packet[1 + used..];
        assert_eq!(&body[..2], &7u16.to_be_bytes());
        assert_eq!(&body[2..4], &9u16.to_be_bytes(), "filter length");
        assert!(body.ends_with(b"sensors/#\x00"));
        assert_eq!(encode_unsubscribe(8, "a/b")[0], 0xA2);
    }

    #[test]
    fn publish_round_trips() {
        let packet = encode_publish("demo/topic", b"hello");
        assert_eq!(packet[0], 0x30);
        let (decoded, used) = decode(&packet).expect("decode").expect("a packet");
        assert_eq!(used, packet.len());
        assert_eq!(
            decoded,
            Packet::Publish {
                topic: "demo/topic".to_owned(),
                payload: b"hello".to_vec(),
                qos: 0,
                retain: false,
                packet_id: 0,
            }
        );
    }

    #[test]
    fn the_encoder_sets_retain_and_qos() {
        let packet = encode_publish_with("a/b", b"x", 1, true, 7);
        // 0x33 = PUBLISH, QoS 1, retain.
        assert_eq!(packet[0], 0x33);
        let (decoded, _) = decode(&packet).expect("decode").expect("a packet");
        assert_eq!(
            decoded,
            Packet::Publish {
                topic: "a/b".to_owned(),
                payload: b"x".to_vec(),
                qos: 1,
                retain: true,
                packet_id: 7,
            }
        );
        // QoS 0 still means no packet id on the wire.
        let plain = encode_publish_with("a/b", b"x", 0, false, 0);
        assert_eq!(plain[0], 0x30);
    }

    #[test]
    fn a_qos_one_publish_carries_its_id() {
        // 0x32 = PUBLISH with QoS 1.
        let mut body = Vec::new();
        put_string(&mut body, "a/b");
        body.extend_from_slice(&9u16.to_be_bytes());
        body.extend_from_slice(b"payload");
        let packet = frame(0x32, body);
        let (decoded, _) = decode(&packet).expect("decode").expect("a packet");
        assert_eq!(
            decoded,
            Packet::Publish {
                topic: "a/b".to_owned(),
                payload: b"payload".to_vec(),
                qos: 1,
                retain: false,
                packet_id: 9,
            }
        );
        assert_eq!(encode_puback(9), vec![0x40, 0x02, 0x00, 0x09]);
    }

    #[test]
    fn connack_reports_acceptance_and_refusals() {
        let accepted = decode(&[0x20, 0x02, 0x00, 0x00]).expect("decode");
        assert_eq!(
            accepted,
            Some((
                Packet::ConnAck {
                    session_present: false,
                    code: 0
                },
                4
            ))
        );
        let refused = decode(&[0x20, 0x02, 0x01, 0x04]).expect("decode");
        assert_eq!(
            refused.map(|(packet, _)| packet),
            Some(Packet::ConnAck {
                session_present: true,
                code: 4
            })
        );
        assert_eq!(Packet::refusal_reason(4), "bad username or password");
    }

    #[test]
    fn partial_packets_wait_for_more_bytes() {
        let packet = encode_publish("a", b"0123456789");
        assert_eq!(decode(&packet[..1]).expect("decode"), None);
        assert_eq!(decode(&packet[..packet.len() - 1]).expect("decode"), None);
        // Two packets in one buffer are decoded one at a time.
        let mut joined = packet.clone();
        joined.extend_from_slice(&[0xD0, 0x00]); // PINGRESP
        let (first, used) = decode(&joined).expect("decode").expect("first");
        assert!(matches!(first, Packet::Publish { .. }));
        let (second, _) = decode(&joined[used..]).expect("decode").expect("second");
        assert_eq!(second, Packet::PingResp);
    }

    #[test]
    fn pings_and_subacks_are_understood() {
        assert_eq!(encode_pingreq(), vec![0xC0, 0x00]);
        assert_eq!(encode_disconnect(), vec![0xE0, 0x00]);
        let (packet, _) = decode(&[0x90, 0x03, 0x00, 0x05, 0x00])
            .expect("decode")
            .expect("a packet");
        assert_eq!(packet, Packet::SubAck { packet_id: 5 });
    }
}
