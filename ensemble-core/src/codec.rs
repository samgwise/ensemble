//! Length-prefixed MessagePack frame codec for async TCP streams.
//!
//! Wire format:
//!   [4 bytes LE length][MessagePack payload]

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::Message;

/// Maximum frame size (1 MiB) to prevent memory exhaustion from malformed data.
const MAX_FRAME_SIZE: u32 = 1024 * 1024;

/// Errors that can occur during frame encoding/decoding.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("MessagePack encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("MessagePack decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("Frame too large: {size} bytes (max {MAX_FRAME_SIZE})")]
    FrameTooLarge { size: u32 },
    #[error("Connection closed")]
    ConnectionClosed,
}

/// Write a message as a length-prefixed MessagePack frame.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &Message,
) -> Result<(), CodecError> {
    let payload = rmp_serde::to_vec_named(msg)?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed MessagePack frame and decode it as a Message.
/// Returns `None` if the connection was cleanly closed (EOF on length read).
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Message, CodecError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(CodecError::ConnectionClosed);
        }
        Err(e) => return Err(CodecError::Io(e)),
    }

    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(CodecError::FrameTooLarge { size: len });
    }

    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;

    let msg: Message = rmp_serde::from_slice(&payload)?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;

    #[tokio::test]
    async fn roundtrip_hello() {
        let msg = Message::Hello(VoiceCapabilities {
            name: "test-voice".into(),
            subscriptions: vec!["/test/*".into()],
            is_bridge: false,
        });

        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = read_message(&mut cursor).await.unwrap();

        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_action() {
        let msg = Message::ActionMessage {
            source: 42,
            action: Action {
                address: "/synth/note".into(),
                signal_type: SignalType::Event,
                timestamp: 0.0,
                payload: Value::Integer(60),
            },
        };

        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = read_message(&mut cursor).await.unwrap();

        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_welcome() {
        let msg = Message::Welcome {
            voice_id: 7,
            hub_time: 12.345,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_goodbye() {
        let msg = Message::Goodbye;
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_clock_sync_request() {
        let msg = Message::ClockSyncRequest {
            voice_send_time: 1.23456,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_clock_sync_reply() {
        let msg = Message::ClockSyncReply {
            voice_send_time: 1.0,
            hub_receive_time: 1.001,
            hub_send_time: 1.002,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_subscribe() {
        let msg = Message::Subscribe {
            patterns: vec!["/midi/*".into(), "/clock".into()],
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_unsubscribe() {
        let msg = Message::Unsubscribe {
            patterns: vec!["/old/*".into()],
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_all_payload_types() {
        let msg = Message::ActionMessage {
            source: 1,
            action: Action {
                address: "/test".into(),
                signal_type: SignalType::Param,
                timestamp: 99.9,
                payload: Value::Tuple(vec![
                    Value::Float(FloatValue::new(0.5)),
                    Value::Integer(-42),
                    Value::Bool(true),
                    Value::String("g'day".into()),
                    Value::Binary(vec![0xDE, 0xAD]),
                ]),
            },
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    #[tokio::test]
    async fn roundtrip_empty_payload() {
        let msg = Message::ActionMessage {
            source: 1,
            action: Action {
                address: "/bang".into(),
                signal_type: SignalType::Event,
                timestamp: 0.0,
                payload: Value::Null,
            },
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).await.unwrap();
        let decoded = read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(msg, decoded);
    }

    // -- Error paths --

    #[tokio::test]
    async fn frame_too_large_is_rejected() {
        // Write a length header that exceeds MAX_FRAME_SIZE.
        let huge_len: u32 = MAX_FRAME_SIZE + 1;
        let buf = huge_len.to_le_bytes().to_vec();
        let result = read_message(&mut std::io::Cursor::new(buf)).await;
        assert!(matches!(result, Err(CodecError::FrameTooLarge { .. })));
    }

    #[tokio::test]
    async fn empty_stream_returns_connection_closed() {
        let buf: Vec<u8> = vec![];
        let result = read_message(&mut std::io::Cursor::new(buf)).await;
        assert!(matches!(result, Err(CodecError::ConnectionClosed)));
    }

    #[tokio::test]
    async fn truncated_length_returns_connection_closed() {
        // Only 2 bytes when 4 are expected for the length header.
        let buf: Vec<u8> = vec![0x01, 0x00];
        let result = read_message(&mut std::io::Cursor::new(buf)).await;
        assert!(matches!(result, Err(CodecError::ConnectionClosed)));
    }

    #[tokio::test]
    async fn multiple_messages_in_sequence() {
        let msgs = vec![
            Message::Goodbye,
            Message::ClockSyncRequest { voice_send_time: 5.0 },
            Message::Hello(VoiceCapabilities {
                name: "multi".into(),
                subscriptions: vec![],
                is_bridge: true,
            }),
        ];

        let mut buf = Vec::new();
        for msg in &msgs {
            write_message(&mut buf, msg).await.unwrap();
        }

        let mut cursor = std::io::Cursor::new(buf);
        for expected in &msgs {
            let decoded = read_message(&mut cursor).await.unwrap();
            assert_eq!(expected, &decoded);
        }
    }
}
