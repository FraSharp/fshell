// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Framed binary codec for client-daemon IPC.
//!
//! Protocol wire format (every packet):
//! ```text
//! ┌─────────────────────┬───────────────────┬─────────────────────┐
//! │  Length (4 bytes)   │  Type (1 byte)    │  Payload (N bytes)  │
//! │  u32 big-endian     │  u8               │  bincode-serialized │
//! └─────────────────────┴───────────────────┴─────────────────────┘
//! ```
//!
//! - `Length` is the length of `Payload` only (excludes the 5-byte header).
//! - `Type` identifies the message variant (see `message::wire`).
//! - `Payload` is the bincode-serialized message body.

use bytes::{Buf, BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

use super::message::{ClientMessage, ServerMessage};

/// Header size: 4 bytes length + 1 byte type = 5 bytes.
const HEADER_SIZE: usize = 5;

/// Maximum payload size (256 MiB). Sanity check to prevent OOM on bad data.
const MAX_PAYLOAD: usize = 256 * 1024 * 1024;

// Framed Types

/// A decoded frame: wire type ID + raw payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub type_id: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Create a new frame from a type ID and payload.
    pub fn new(type_id: u8, payload: Vec<u8>) -> Self {
        Self { type_id, payload }
    }

    /// Serialize a `ClientMessage` into a frame.
    pub fn from_client(msg: &ClientMessage) -> Self {
        let type_id = msg.wire_type();
        let payload = bincode::serialize(msg).expect("ClientMessage serialization failed");
        Self::new(type_id, payload)
    }

    /// Serialize a `ServerMessage` into a frame.
    pub fn from_server(msg: &ServerMessage) -> Self {
        let type_id = msg.wire_type();
        let payload = bincode::serialize(msg).expect("ServerMessage serialization failed");
        Self::new(type_id, payload)
    }

    /// Deserialize the payload as a `ClientMessage`.
    pub fn into_client(self) -> Result<ClientMessage, bincode::Error> {
        ClientMessage::from_wire(self.type_id, &self.payload)
    }

    /// Deserialize the payload as a `ServerMessage`.
    pub fn into_server(self) -> Result<ServerMessage, bincode::Error> {
        ServerMessage::from_wire(self.type_id, &self.payload)
    }
}

// FshCodec

/// Tokio codec implementing the framed binary protocol.
///
/// Works bidirectionally: encodes `Frame` → `BytesMut` and
/// decodes `BytesMut` → `Frame`.
pub struct FshCodec;

impl Decoder for FshCodec {
    type Item = Frame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, io::Error> {
        // Need at least the header before we can read the length.
        if src.len() < HEADER_SIZE {
            return Ok(None);
        }

        // Peek at the length without consuming (we need to verify
        // we have enough bytes for the full payload).
        let payload_len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;

        // Sanity check: reject absurdly large payloads.
        if payload_len > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "payload too large: {} bytes (max {})",
                    payload_len, MAX_PAYLOAD
                ),
            ));
        }

        // Wait for the full frame to arrive.
        let total = HEADER_SIZE + payload_len;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }

        // We have the full frame. Consume it.
        src.advance(4); // skip length
        let type_id = src[0];
        src.advance(1); // skip type
        let payload = src.split_to(payload_len).to_vec();

        Ok(Some(Frame::new(type_id, payload)))
    }
}

impl Encoder<Frame> for FshCodec {
    type Error = io::Error;

    fn encode(&mut self, item: Frame, dst: &mut BytesMut) -> Result<(), io::Error> {
        let total = HEADER_SIZE + item.payload.len();
        dst.reserve(total);

        // Write length (big-endian u32).
        dst.put_u32(item.payload.len() as u32);
        // Write type.
        dst.put_u8(item.type_id);
        // Write payload.
        dst.put_slice(&item.payload);

        Ok(())
    }
}

// Convenience: Direct Message Encode/Decode

/// Encode a `ClientMessage` directly into a `BytesMut` buffer.
pub fn encode_client_message(msg: &ClientMessage, dst: &mut BytesMut) -> Result<(), io::Error> {
    FshCodec.encode(Frame::from_client(msg), dst)
}

/// Encode a `ServerMessage` directly into a `BytesMut` buffer.
pub fn encode_server_message(msg: &ServerMessage, dst: &mut BytesMut) -> Result<(), io::Error> {
    FshCodec.encode(Frame::from_server(msg), dst)
}

#[cfg(test)]
mod tests {
    use super::super::message::*;
    use super::*;

    #[test]
    fn codec_roundtrip_client_message() {
        let msg = ClientMessage::Resize {
            cols: 120,
            rows: 40,
        };
        let frame = Frame::from_client(&msg);

        let mut buf = BytesMut::new();
        FshCodec.encode(frame, &mut buf).unwrap();

        let decoded = FshCodec.decode(&mut buf).unwrap().unwrap();
        let reconstructed = decoded.into_client().unwrap();
        assert_eq!(reconstructed, msg);
    }

    #[test]
    fn codec_roundtrip_server_message() {
        let msg = ServerMessage::Draw(vec![0x1b, 0x5b, 0x32, 0x4a]);
        let frame = Frame::from_server(&msg);

        let mut buf = BytesMut::new();
        FshCodec.encode(frame, &mut buf).unwrap();

        let decoded = FshCodec.decode(&mut buf).unwrap().unwrap();
        let reconstructed = decoded.into_server().unwrap();
        assert_eq!(reconstructed, msg);
    }

    #[test]
    fn codec_partial_frame_returns_none() {
        let msg = ClientMessage::Detach;
        let frame = Frame::from_client(&msg);

        let mut buf = BytesMut::new();
        FshCodec.encode(frame, &mut buf).unwrap();

        // Simulate partial read: take only first 3 bytes.
        let mut partial = buf.split_to(3);
        assert!(FshCodec.decode(&mut partial).unwrap().is_none());
    }

    #[test]
    fn codec_multiple_frames_in_buffer() {
        let msg1 = ClientMessage::Detach;
        let msg2 = ClientMessage::ListSessions;

        let mut buf = BytesMut::new();
        FshCodec
            .encode(Frame::from_client(&msg1), &mut buf)
            .unwrap();
        FshCodec
            .encode(Frame::from_client(&msg2), &mut buf)
            .unwrap();

        let decoded1 = FshCodec.decode(&mut buf).unwrap().unwrap();
        let decoded2 = FshCodec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(decoded1.into_client().unwrap(), msg1);
        assert_eq!(decoded2.into_client().unwrap(), msg2);
        assert!(buf.is_empty());
    }

    #[test]
    fn codec_rejects_oversized_payload() {
        let mut buf = BytesMut::new();
        // Write a length that exceeds MAX_PAYLOAD.
        buf.put_u32((MAX_PAYLOAD as u32) + 1);
        buf.put_u8(0x01);
        buf.put_slice(&[0u8; 100]);

        let result = FshCodec.decode(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn frame_header_is_exactly_5_bytes() {
        let msg = ClientMessage::Input(vec![1, 2, 3]);
        let frame = Frame::from_client(&msg);

        let mut buf = BytesMut::new();
        FshCodec.encode(frame, &mut buf).unwrap();

        // Header is always 5 bytes; total = 5 + payload_len
        // payload_len is written as the first 4 bytes of the frame
        let payload_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        assert_eq!(buf.len(), HEADER_SIZE + payload_len);
    }
}
