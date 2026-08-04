//! Discord IPC wire transport: the 8-byte frame codec, opcodes, handshake,
//! and the READY response.
//!
//! The payload model for frames is `rsrpc::cmd` (the battle-tested types from
//! the MIT-licensed rsRPC crate) - this module only carries the transport
//! pieces rsRPC keeps crate-private (`server::ipc_utils`, `server::utils`).

use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Upper bound on one frame's payload. Generous for Rich Presence JSON;
/// guards against a corrupt length header eating memory.
pub const MAX_PAYLOAD_SIZE: usize = 1024 * 1024;

/// The READY dispatch Discord-compatible servers answer the handshake with.
/// Mirrors arRPC/rsRPC's `CONNECTION_RESPONSE`; the user block is a dummy -
/// games only require the envelope shape.
pub const CONNECTION_RESPONSE: &str = r#"{
  "cmd": "DISPATCH",
  "evt": "READY",
  "data": {
    "v": 1,
    "user": {
      "id": "1045800378228281345",
      "username": "gamebus-presenced",
      "discriminator": "0000",
      "avatar": null,
      "flags": 0,
      "premium_type": 0
    },
    "config": {
      "api_endpoint": "//discord.com/api",
      "cdn_host": "cdn.discordapp.com",
      "environment": "production"
    }
  }
}"#;

/// Discord IPC opcodes (same numbering as rsRPC's `PacketType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PacketType {
    Handshake = 0,
    Frame = 1,
    Close = 2,
    Ping = 3,
    Pong = 4,
}

impl TryFrom<u32> for PacketType {
    type Error = std::io::Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Handshake),
            1 => Ok(Self::Frame),
            2 => Ok(Self::Close),
            3 => Ok(Self::Ping),
            4 => Ok(Self::Pong),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown Discord IPC opcode {value}"),
            )),
        }
    }
}

/// One wire frame: `u32 LE opcode` + `u32 LE payload length` + payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub packet_type: PacketType,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(packet_type: PacketType, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            packet_type,
            payload: payload.into(),
        }
    }

    /// Serialise as `opcode | length | payload`, little-endian header.
    pub fn encode(&self) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(8 + self.payload.len());
        buffer.extend_from_slice(&(self.packet_type as u32).to_le_bytes());
        buffer.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&self.payload);
        buffer
    }

    /// Read one frame from an async stream.
    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Self> {
        let opcode = reader.read_u32_le().await?;
        let len = reader.read_u32_le().await? as usize;
        if len > MAX_PAYLOAD_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Discord IPC payload too large: {len}"),
            ));
        }
        let mut payload = vec![0; len];
        reader.read_exact(&mut payload).await?;
        Ok(Self {
            packet_type: PacketType::try_from(opcode)?,
            payload,
        })
    }
}

/// The client's opening handshake (opcode 0).
#[derive(Debug, Deserialize)]
pub struct Handshake {
    pub v: u32,
    pub client_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip() {
        let expected = Frame::new(PacketType::Frame, br#"{"cmd":"SET_ACTIVITY"}"#.to_vec());
        let encoded = expected.encode();
        let actual = Frame::read_from(&mut encoded.as_slice()).await.unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn rejects_oversized_payload() {
        let mut header = Vec::new();
        header.extend_from_slice(&(PacketType::Frame as u32).to_le_bytes());
        header.extend_from_slice(&((MAX_PAYLOAD_SIZE + 1) as u32).to_le_bytes());
        let error = Frame::read_from(&mut header.as_slice()).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn handshake_parses() {
        let hs: Handshake = serde_json::from_str(r#"{"v":1,"client_id":"123456789"}"#).unwrap();
        assert_eq!(hs.v, 1);
        assert_eq!(hs.client_id, "123456789");
    }
}
