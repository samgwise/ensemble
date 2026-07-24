//! QUIC transport for remote peer connections.
//!
//! Handles both inbound (listener) and outbound (connector) QUIC connections
//! with self-signed TLS certificates. Each peer connection opens a single
//! bidirectional QUIC stream for reliable messages (control, param, event)
//! and uses QUIC datagrams for stream-type actions (best-effort).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use ensemble_core::codec;
use ensemble_core::protocol::*;
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::PeerConfig;
use crate::loop_guard::LoopGuard;
use crate::mapping::{Direction, MappingEngine};
use crate::protocol;

// ---------------------------------------------------------------------------
// TLS configuration
// ---------------------------------------------------------------------------

/// Generate a self-signed certificate for QUIC TLS.
fn generate_self_signed_cert() -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .context("failed to generate self-signed certificate")?;

    let cert_der = CertificateDer::from(cert.cert);
    let key_der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));

    Ok((vec![cert_der], key_der))
}

/// Create a QUIC server configuration with self-signed TLS.
fn create_server_config() -> Result<ServerConfig> {
    let (certs, key) = generate_self_signed_cert()?;

    let mut server_config = ServerConfig::with_single_cert(certs, key)
        .context("failed to create server config with certificate")?;

    // Allow migration for better connection resilience.
    server_config.migration(true);

    // Enable datagram reception.
    let transport = quinn::TransportConfig::default();
    // Note: datagram_receive_window is set via the endpoint, not transport config.
    server_config.transport_config(Arc::new(transport));

    Ok(server_config)
}

/// Create a QUIC client configuration that accepts any certificate.
fn create_client_config() -> ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();

    ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap(),
    ))
}

/// Certificate verifier that accepts any certificate (for development).
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}

// ---------------------------------------------------------------------------
// Peer connection
// ---------------------------------------------------------------------------

/// A connected remote peer (before session setup).
#[derive(Debug, Clone)]
pub struct RemotePeer {
    /// QUIC connection.
    pub connection: Connection,
    /// Peer address.
    pub addr: SocketAddr,
    /// Whether this is an inbound connection.
    pub is_inbound: bool,
}

/// Identifying information for a remote bridge peer.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Remote bridge ID.
    pub bridge_id: String,
    /// Remote bridge name.
    pub name: String,
}

/// Start a QUIC listener on the specified port.
///
/// Returns the actual port bound to. The accept loop is spawned through the
/// provided `TaskTracker` and returns immediately once the endpoint is bound.
/// When `cancel` is triggered, the listener stops accepting and the endpoint
/// is dropped, releasing the UDP socket and closing all connections on it.
pub async fn start_listener(
    port: u16,
    inbound_tx: mpsc::Sender<RemotePeer>,
    cancel: CancellationToken,
    tracker: TaskTracker,
) -> Result<u16> {
    let server_config = create_server_config()?;
    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;

    let endpoint = Endpoint::server(server_config, addr).context("failed to bind QUIC endpoint")?;
    let actual_port = endpoint.local_addr()?.port();

    eprintln!("QUIC listener ready on {}", endpoint.local_addr()?);

    // Accept incoming connections.
    tracker.spawn(async move {
        loop {
            tokio::select! {
                incoming = endpoint.accept() => {
                    match incoming {
                        Some(incoming) => {
                            let inbound_tx = inbound_tx.clone();

                            tokio::spawn(async move {
                                match incoming.await {
                                    Ok(connection) => {
                                        let addr = connection.remote_address();
                                        eprintln!("Inbound QUIC connection from {}", addr);

                                        let peer = RemotePeer {
                                            connection,
                                            addr,
                                            is_inbound: true,
                                        };

                                        if let Err(e) = inbound_tx.send(peer).await {
                                            eprintln!("Failed to register inbound peer: {}", e);
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to accept QUIC connection: {}", e);
                                    }
                                }
                            });
                        }
                        None => break,
                    }
                }
                _ = cancel.cancelled() => {
                    eprintln!("QUIC listener shutting down");
                    break;
                }
            }
        }
        // Cease accepting and close any connections on this endpoint, then drop
        // it so quinn's internal driver releases the UDP socket. (The kernel
        // release is asynchronous; callers that need to rebind should retry.)
        endpoint.close(0u32.into(), b"bridge shutdown");
        drop(endpoint);
    });

    Ok(actual_port)
}

/// Connect to a remote peer.
#[allow(dead_code)]
pub async fn connect_to_peer(peer_config: &PeerConfig) -> Result<RemotePeer> {
    let client_config = create_client_config();

    // Bind to any available local port.
    let mut endpoint =
        Endpoint::client("0.0.0.0:0".parse()?).context("failed to create client endpoint")?;
    endpoint.set_default_client_config(client_config);

    let addr: SocketAddr = format!("{}:{}", peer_config.host, peer_config.port)
        .parse()
        .context("invalid peer address")?;

    eprintln!("Connecting to peer at {}...", addr);

    let connection = endpoint
        .connect(addr, "localhost")?
        .await
        .context("failed to establish QUIC connection")?;

    eprintln!("Connected to peer at {}", addr);

    Ok(RemotePeer {
        connection,
        addr,
        is_inbound: false,
    })
}

// ---------------------------------------------------------------------------
// Peer session — full bidirectional forwarding
// ---------------------------------------------------------------------------

/// Shared state for dispatching inbound actions to the local hub.
pub struct HubSink {
    /// Sender for actions to publish to the local hub.
    pub tx: mpsc::Sender<WireMessage>,
}

/// Perform the bridge handshake on a new QUIC connection.
///
/// The outbound peer opens a bidirectional stream, sends its hello, and reads
/// the peer's reply. The inbound peer accepts the incoming stream, reads the
/// peer's hello, and replies. This avoids a deadlock where both sides open
/// separate streams and wait for each other.
pub async fn handshake(
    peer: &RemotePeer,
    local_bridge_id: &str,
    local_name: &str,
) -> Result<(PeerInfo, quinn::SendStream, quinn::RecvStream)> {
    let hello = protocol::bridge_hello(local_bridge_id, local_name);

    if peer.is_inbound {
        let (mut send, mut recv) = peer
            .connection
            .accept_bi()
            .await
            .context("failed to accept QUIC stream")?;
        let peer_hello = codec::read_message(&mut recv)
            .await
            .context("failed to read bridge_hello")?;
        codec::write_message(&mut send, &hello)
            .await
            .context("failed to send bridge_hello")?;
        return parse_peer_hello(peer_hello, send, recv);
    }

    let (mut send, mut recv) = peer
        .connection
        .open_bi()
        .await
        .context("failed to open QUIC stream")?;
    codec::write_message(&mut send, &hello)
        .await
        .context("failed to send bridge_hello")?;
    let peer_hello = codec::read_message(&mut recv)
        .await
        .context("failed to read bridge_hello")?;
    parse_peer_hello(peer_hello, send, recv)
}

fn parse_peer_hello(
    peer_hello: WireMessage,
    send_stream: quinn::SendStream,
    recv_stream: quinn::RecvStream,
) -> Result<(PeerInfo, quinn::SendStream, quinn::RecvStream)> {
    if peer_hello.msg_type != protocol::MSG_BRIDGE_HELLO {
        anyhow::bail!("expected bridge_hello, got {}", peer_hello.msg_type);
    }

    let payload = match peer_hello.payload {
        Value::Map(m) => m,
        _ => anyhow::bail!("bridge_hello payload is not a map"),
    };

    let peer_bridge_id = match payload.get("bridge_id") {
        Some(Value::String(s)) => s.clone(),
        _ => "unknown".to_string(),
    };
    let peer_name = match payload.get("name") {
        Some(Value::String(s)) => s.clone(),
        _ => "unknown".to_string(),
    };

    Ok((
        PeerInfo {
            bridge_id: peer_bridge_id,
            name: peer_name,
        },
        send_stream,
        recv_stream,
    ))
}
/// Run a full peer session using the provided pre-handshaked streams.
///
/// This function takes ownership of the connection and streams, and runs until
/// the peer disconnects, an error occurs, or the `cancel` token is triggered
/// (e.g. bridge shutdown). The writer and reader sub-tasks are awaited before
/// returning, so the tracked session task does not complete until both have
/// exited.
///
/// The `replay_rx` channel receives param replay messages that should be sent
/// to this peer before live outbound traffic (e.g. current param state when a
/// new peer connects).
#[allow(clippy::too_many_arguments)]
pub async fn run_peer_session(
    peer: RemotePeer,
    peer_info: PeerInfo,
    mut send_stream: quinn::SendStream,
    mut recv_stream: quinn::RecvStream,
    loop_guard: Arc<LoopGuard>,
    engine: Arc<MappingEngine>,
    hub_sink: Arc<HubSink>,
    mut outbound_rx: broadcast::Receiver<WireMessage>,
    mut replay_rx: mpsc::Receiver<WireMessage>,
    cancel: CancellationToken,
) {
    let addr = peer.addr;
    let connection = peer.connection;

    eprintln!(
        "[peer {}] Handshake complete — peer: \"{}\" (bridge_id: {})",
        addr, peer_info.name, peer_info.bridge_id
    );

    // A child token scoped to this session: cancelling it stops the writer and
    // reader sub-tasks without affecting the rest of the bridge, and a bridge
    // shutdown (parent cancel) cascades here automatically.
    let session_cancel = cancel.child_token();

    // Split the session into reader and writer tasks.
    let conn_writer = connection.clone();
    let conn_reader = connection.clone();
    let lg_reader = loop_guard.clone();
    let engine_reader = engine.clone();
    let sink_reader = hub_sink.clone();

    // Writer and reader sub-tasks are tracked in a JoinSet so we can wait for
    // the first to finish, then cancel the session and join the rest — without
    // re-polling a completed handle (which panics in current tokio).
    let mut set = tokio::task::JoinSet::new();

    // Writer task: outbound broadcast + replay → QUIC stream + datagrams.
    let writer_cancel = session_cancel.clone();
    set.spawn(async move {
        let mut replay_closed = false;
        loop {
            let msg = if replay_closed {
                tokio::select! {
                    _ = writer_cancel.cancelled() => break,
                    result = outbound_rx.recv() => {
                        match result {
                            Ok(msg) => msg,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                eprintln!("[peer {}] Writer lagged, skipped {} messages", addr, n);
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                eprintln!("[peer {}] Outbound channel closed", addr);
                                break;
                            }
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = writer_cancel.cancelled() => break,
                    result = outbound_rx.recv() => {
                        match result {
                            Ok(msg) => msg,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                eprintln!("[peer {}] Writer lagged, skipped {} messages", addr, n);
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                eprintln!("[peer {}] Outbound channel closed", addr);
                                break;
                            }
                        }
                    }
                    result = replay_rx.recv() => {
                        match result {
                            Some(msg) => msg,
                            None => {
                                replay_closed = true;
                                continue;
                            }
                        }
                    }
                }
            };

            // Determine signal type for routing to stream vs datagram.
            let signal_type =
                protocol::get_signal_type(&msg).unwrap_or_else(|| "event".to_string());

            if signal_type == "stream" {
                // Send as QUIC datagram (best-effort, lowest latency).
                match codec::encode_to_vec(&msg) {
                    Ok(bytes) => {
                        if let Err(e) = conn_writer.send_datagram(bytes.into()) {
                            eprintln!("[peer {}] Datagram send failed: {}", addr, e);
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[peer {}] Datagram encode failed: {}", addr, e);
                    }
                }
            } else {
                // Send on the reliable stream.
                if let Err(e) = codec::write_message(&mut send_stream, &msg).await {
                    eprintln!("[peer {}] Stream write failed: {}", addr, e);
                    break;
                }
            }
        }
        // Close the send stream.
        let _ = send_stream.finish();
    });

    // Reader task: QUIC stream + datagrams → inbound to local hub.
    let reader_cancel = session_cancel.clone();
    set.spawn(async move {
        // Spawn a sub-task for datagram reception.
        let sink_dgram = sink_reader.clone();
        let engine_dgram = engine_reader.clone();
        let lg_dgram = lg_reader.clone();
        let dgram_addr = addr;
        let dgram_cancel = reader_cancel.clone();
        let dgram_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = dgram_cancel.cancelled() => break,
                    result = conn_reader.read_datagram() => {
                        match result {
                            Ok(bytes) => match codec::decode_from_slice(&bytes) {
                                Ok(msg) => {
                                    handle_inbound_message(
                                        msg,
                                        dgram_addr,
                                        &lg_dgram,
                                        &engine_dgram,
                                        &sink_dgram,
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    eprintln!("[peer {}] Datagram decode failed: {}", dgram_addr, e);
                                }
                            }
                            Err(e) => {
                                eprintln!("[peer {}] Datagram read failed: {}", dgram_addr, e);
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Read reliable messages from the bidirectional stream.
        loop {
            tokio::select! {
                _ = reader_cancel.cancelled() => break,
                result = codec::read_message(&mut recv_stream) => {
                    match result {
                        Ok(msg) => {
                            handle_inbound_message(msg, addr, &lg_reader, &engine_reader, &sink_reader)
                                .await;
                        }
                        Err(codec::CodecError::ConnectionClosed) => {
                            eprintln!("[peer {}] Stream closed", addr);
                            break;
                        }
                        Err(e) => {
                            eprintln!("[peer {}] Stream read error: {}", addr, e);
                            break;
                        }
                    }
                }
            }
        }

        dgram_handle.abort();
    });

    // Wait for the first sub-task to finish (peer disconnect or shutdown),
    // then cancel the session so the other stops, and join the remainder. The
    // JoinSet drains both sub-tasks so the tracked session task does not return
    // while a sub-task lingers, and it never re-polls a completed handle.
    let _ = set.join_next().await;
    session_cancel.cancel();
    set.join_all().await;

    // Close the connection.
    connection.close(0u32.into(), b"session ended");
    eprintln!("[peer {}] Session ended", addr);
}

/// Handle an inbound bridge message: check loop guard, map address, forward to hub.
async fn handle_inbound_message(
    msg: WireMessage,
    peer_addr: SocketAddr,
    loop_guard: &LoopGuard,
    engine: &MappingEngine,
    sink: &HubSink,
) {
    match msg.msg_type.as_str() {
        protocol::MSG_BRIDGE_ACTION => {
            // Check loop guard.
            if let Some(origin) = protocol::get_origin(&msg) {
                if loop_guard.is_loop(&origin) {
                    eprintln!(
                        "[peer {}] Dropping looped action (origin={})",
                        peer_addr, origin
                    );
                    return;
                }
            }

            // Map address and forward to local hub.
            let address = protocol::get_address(&msg).unwrap_or_default();
            let signal_type_str =
                protocol::get_signal_type(&msg).unwrap_or_else(|| "event".to_string());

            if let Some(mapped_address) =
                engine.map(&address, Direction::Inbound, Some(&signal_type_str))
            {
                let source = protocol::get_source(&msg);
                let timestamp = protocol::get_timestamp(&msg);
                let payload = protocol::get_action_payload(&msg);

                // Parse signal type.
                let signal_type = match signal_type_str.as_str() {
                    "param" => SignalType::Param,
                    "stream" => SignalType::Stream,
                    _ => SignalType::Event,
                };

                let action =
                    action_with_source(source, &mapped_address, signal_type, timestamp, payload);

                eprintln!(
                    "[peer {}] Inbound: {} → {} (source={})",
                    peer_addr, address, mapped_address, source
                );

                if let Err(e) = sink.tx.send(action).await {
                    eprintln!("[peer {}] Failed to forward to hub: {}", peer_addr, e);
                }
            }
        }
        protocol::MSG_BRIDGE_HELLO => {
            // Duplicate hello — ignore.
        }
        protocol::MSG_BRIDGE_PING => {
            // TODO: Reply with pong.
        }
        other => {
            eprintln!("[peer {}] Unknown message type: {}", peer_addr, other);
        }
    }
}
