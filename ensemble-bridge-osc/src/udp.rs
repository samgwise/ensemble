//! UDP sender and receiver tasks for OSC communication.
//!
//! The sender runs on a dedicated std thread (like the MIDI bridge) because
//! UdpSocket operations are blocking. The listener also runs on a std thread.

use std::net::UdpSocket;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc;

use rosc::{OscPacket, OscMessage, encoder};
use rosc::decoder::decode_udp;

/// Command sent to the UDP sender task.
pub enum UdpSendCmd {
    /// Send an OSC message to the target.
    Send(OscMessage),
}

/// Spawn a UDP sender task on a dedicated std thread.
///
/// Returns a channel sender for queuing messages to send.
pub fn spawn_udp_sender(
    socket: UdpSocket,
    target_addr: String,
) -> mpsc::Sender<UdpSendCmd> {
    let (tx, mut rx) = mpsc::channel::<UdpSendCmd>(256);

    // rosc's encoder and UdpSocket are not async-friendly, so we use a std thread.
    std::thread::spawn(move || {
        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                UdpSendCmd::Send(msg) => {
                    let packet = OscPacket::Message(msg);
                    match encoder::encode(&packet) {
                        Ok(bytes) => {
                            if let Err(e) = socket.send_to(&bytes, &target_addr) {
                                eprintln!("UDP send error: {}", e);
                            }
                        }
                        Err(e) => {
                            eprintln!("OSC encode error: {}", e);
                        }
                    }
                }
            }
        }
    });

    tx
}

/// Message received from the UDP listener.
pub struct UdpReceivedMsg {
    /// The OSC message that was received.
    pub message: OscMessage,
    /// The source address of the sender.
    pub src_addr: String,
}

/// Spawn a UDP listener task on a dedicated std thread.
///
/// Returns a channel receiver for incoming OSC messages.
pub fn spawn_udp_listener(
    listen_port: u16,
) -> anyhow::Result<std_mpsc::Receiver<UdpReceivedMsg>> {
    let socket = UdpSocket::bind(format!("127.0.0.1:{}", listen_port))?;
    socket.set_nonblocking(false)?;

    let (tx, rx) = std_mpsc::channel::<UdpReceivedMsg>();

    std::thread::spawn(move || {
        let mut buf = [0u8; rosc::decoder::MTU];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((size, src)) => {
                    match decode_udp(&buf[..size]) {
                        Ok((_bytes_read, OscPacket::Message(msg))) => {
                            let received = UdpReceivedMsg {
                                message: msg,
                                src_addr: src.to_string(),
                            };
                            if tx.send(received).is_err() {
                                // Receiver dropped, exit.
                                break;
                            }
                        }
                        Ok((_bytes_read, OscPacket::Bundle(bundle))) => {
                            // v1: ignore bundles, but log them.
                            eprintln!(
                                "Received OSC bundle from {} ({} messages) — bundles not yet supported",
                                src,
                                bundle.content.len()
                            );
                        }
                        Err(e) => {
                            eprintln!("OSC decode error from {}: {}", src, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("UDP recv error: {}", e);
                    // Continue listening; transient errors shouldn't kill the listener.
                }
            }
        }
    });

    Ok(rx)
}
