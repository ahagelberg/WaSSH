//! A small MQTT 3.1.1 client for the message analyser.
//!
//! The analyser only needs to watch a topic and occasionally publish, so this crate carries its
//! own packet codec instead of a full broker library: connect, subscribe, publish at QoS 0,
//! acknowledge QoS 1 messages, keep alive, disconnect.

#![forbid(unsafe_code)]

pub mod client;
pub mod codec;

pub use client::{MqttEvent, MqttOptions, MqttSession};
pub use codec::Packet;
