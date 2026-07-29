//! QUIC transport for remote peer connections.
//!
//! Handles both inbound (listener) and outbound (connector) QUIC connections
//! with self-signed TLS certificates. Each peer connection opens a single
//! bidirectional QUIC stream for reliable messages (control, param, event)
//! and uses QUIC datagrams for stream-type actions (best-effort).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Start a QUIC listener on the specified address and port.
///
/// `listen_addr` may be an IPv4 or IPv6 literal (brackets optional for IPv6)
/// or a resolvable hostname. Accepted inbound connections are counted
/// against `inbound_gauge`; once `max_inbound` connections are open, further
/// connections are closed immediately. The peer manager decrements the
/// gauge as sessions end.
///
/// Returns the actual port bound to. The accept loop is spawned through the
/// provided `TaskTracker` and returns immediately once the endpoint is bound.
/// When `cancel` is triggered, the listener stops accepting and the endpoint
/// is dropped, releasing the UDP socket and closing all connections on it.
#[allow(clippy::too_many_arguments)]
pub async fn start_listener(
    listen_addr: &str,
    port: u16,
    inbound_tx: mpsc::Sender<RemotePeer>,
    inbound_gauge: Arc<AtomicUsize>,
    max_inbound: usize,
    cancel: CancellationToken,
    tracker: TaskTracker,
) -> Result<u16> {
    let server_config = create_server_config()?;
    let addr = resolve_bind_addr(listen_addr, port).await?;

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
                            let gauge = inbound_gauge.clone();

                            tokio::spawn(async move {
                                match incoming.await {
                                    Ok(connection) => {
                                        let addr = connection.remote_address();

                                        // Enforce the inbound cap before
                                        // committing resources to the session.
                                        if gauge.fetch_add(1, Ordering::SeqCst) >= max_inbound {
                                            eprintln!(
                                                "Inbound connection from {} rejected: cap of {} reached",
                                                addr, max_inbound
                                            );
                                            connection.close(0u32.into(), b"connection limit reached");
                                            gauge.fetch_sub(1, Ordering::SeqCst);
                                            return;
                                        }
                                        eprintln!("Inbound QUIC connection from {}", addr);

                                        let peer = RemotePeer {
                                            connection,
                                            addr,
                                            is_inbound: true,
                                        };

                                        if let Err(e) = inbound_tx.send(peer).await {
                                            eprintln!("Failed to register inbound peer: {}", e);
                                            gauge.fetch_sub(1, Ordering::SeqCst);
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

/// Format a host and port for parsing or resolution, adding brackets around
/// bare IPv6 literals.
fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Resolve a configured listen address to a socket address.
///
/// Accepts IPv4/IPv6 literals (brackets optional for IPv6) and hostnames;
/// the first resolved address is used.
async fn resolve_bind_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let candidate = format_host_port(host, port);
    if let Ok(addr) = candidate.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let mut addrs = tokio::net::lookup_host(&candidate)
        .await
        .with_context(|| format!("failed to resolve listen address '{host}'"))?;
    addrs
        .next()
        .with_context(|| format!("listen address '{host}' resolved to no addresses"))
}

/// Connect to a remote peer.
///
/// The configured host is resolved through the system resolver, so DNS
/// hostnames and IPv6 literals work as well as IPv4 addresses. Each
/// resolved address is tried in turn until a connection succeeds.
pub async fn connect_to_peer(peer_config: &PeerConfig) -> Result<RemotePeer> {
    let candidate = format_host_port(&peer_config.host, peer_config.port);
    let addrs: Vec<SocketAddr> = match candidate.parse::<SocketAddr>() {
        Ok(addr) => vec![addr],
        Err(_) => {
            let resolved: Vec<SocketAddr> = tokio::net::lookup_host(&candidate)
                .await
                .with_context(|| format!("failed to resolve peer host '{}'", peer_config.host))?
                .collect();
            if resolved.is_empty() {
                anyhow::bail!("peer host '{}' resolved to no addresses", peer_config.host);
            }
            resolved
        }
    };

    let mut last_err: Option<anyhow::Error> = None;
    for addr in addrs {
        match connect_to_addr(addr).await {
            Ok(peer) => return Ok(peer),
            Err(e) => {
                eprintln!("Connect to {} failed: {}", addr, e);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no peer addresses to try")))
}

/// Establish a QUIC connection to a single resolved socket address.
async fn connect_to_addr(addr: SocketAddr) -> Result<RemotePeer> {
    let client_config = create_client_config();

    // Bind the local endpoint on the same address family as the target so
    // both IPv4 and IPv6 peers are reachable.
    let bind: SocketAddr = match addr {
        SocketAddr::V4(_) => "0.0.0.0:0".parse()?,
        SocketAddr::V6(_) => "[::]:0".parse()?,
    };
    let mut endpoint = Endpoint::client(bind).context("failed to create client endpoint")?;
    endpoint.set_default_client_config(client_config);

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
///
/// When `auth_token` is `Some`, the peer must present the identical token in
/// its hello; the comparison is constant-time. On mismatch the connection is
/// closed with an "authentication failed" reason and the handshake errors.
/// The inbound side verifies the token before sending its own hello, so an
/// unauthenticated peer learns nothing about us.
pub async fn handshake(
    peer: &RemotePeer,
    local_bridge_id: &str,
    local_name: &str,
    auth_token: Option<&str>,
) -> Result<(PeerInfo, quinn::SendStream, quinn::RecvStream)> {
    let hello = protocol::bridge_hello(local_bridge_id, local_name, auth_token);

    if peer.is_inbound {
        let (mut send, mut recv) = peer
            .connection
            .accept_bi()
            .await
            .context("failed to accept QUIC stream")?;
        let peer_hello = codec::read_message(&mut recv)
            .await
            .context("failed to read bridge_hello")?;
        let presented = protocol::get_auth_token(&peer_hello);
        if !protocol::auth_token_matches(auth_token, presented.as_deref()) {
            peer.connection.close(0u32.into(), b"authentication failed");
            anyhow::bail!("peer rejected: invalid auth token");
        }
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
    let presented = protocol::get_auth_token(&peer_hello);
    if !protocol::auth_token_matches(auth_token, presented.as_deref()) {
        peer.connection.close(0u32.into(), b"authentication failed");
        anyhow::bail!("peer rejected: invalid auth token");
    }
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
/// Send one outbound message to the peer, as a datagram for stream signals
/// or on the reliable stream otherwise.
///
/// Returns `false` when the session should end (connection-level failure).
/// Datagrams that fail for congestion, size or capability reasons are logged
/// and dropped without harming the session.
async fn send_outbound_message(
    msg: &WireMessage,
    send_stream: &mut quinn::SendStream,
    conn: &quinn::Connection,
    addr: SocketAddr,
) -> bool {
    let signal_type = protocol::get_signal_type(msg).unwrap_or_else(|| "event".to_string());

    if signal_type == "stream" {
        // Send as QUIC datagram (best-effort, lowest latency).
        match codec::encode_to_vec(msg) {
            Ok(bytes) => {
                if let Err(e) = conn.send_datagram(bytes.into()) {
                    match e {
                        quinn::SendDatagramError::ConnectionLost(e) => {
                            eprintln!("[peer {}] Connection lost on datagram send: {}", addr, e);
                            return false;
                        }
                        other => {
                            // Congestion, size or capability failures drop this
                            // datagram only — the session carries on.
                            eprintln!("[peer {}] Datagram dropped: {}", addr, other);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[peer {}] Datagram encode failed: {}", addr, e);
            }
        }
        return true;
    }

    // Send on the reliable stream.
    if let Err(e) = codec::write_message(send_stream, msg).await {
        eprintln!("[peer {}] Stream write failed: {}", addr, e);
        return false;
    }
    true
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
/// new peer connects). The replay is drained fully before any live message is
/// sent, so the peer sees a consistent state snapshot ahead of updates.
///
/// `reforward_tx` is the bridge-wide outbound broadcast; inbound messages
/// that pass the loop guard are re-forwarded through it (unchanged) so they
/// propagate to other peers across multi-hop topologies.
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
    reforward_tx: broadcast::Sender<WireMessage>,
    replay_rx: mpsc::Receiver<WireMessage>,
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

    // Writer task: param replay, then live outbound broadcast → QUIC stream +
    // datagrams.
    let writer_cancel = session_cancel.clone();
    set.spawn(async move {
        let mut replay_rx = replay_rx;

        // Drain the param replay fully before any live traffic, so the peer
        // receives a consistent state snapshot before updates. When replay is
        // disabled the sender is already dropped and this loop exits at once.
        loop {
            tokio::select! {
                _ = writer_cancel.cancelled() => {
                    let _ = send_stream.finish();
                    return;
                }
                msg = replay_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if !send_outbound_message(&msg, &mut send_stream, &conn_writer, addr).await {
                                let _ = send_stream.finish();
                                return;
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        // Live traffic.
        loop {
            let msg = tokio::select! {
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
            };

            if !send_outbound_message(&msg, &mut send_stream, &conn_writer, addr).await {
                break;
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
        let reforward_dgram = reforward_tx.clone();
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
                                        &reforward_dgram,
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
                            handle_inbound_message(
                                msg,
                                addr,
                                &lg_reader,
                                &engine_reader,
                                &sink_reader,
                                &reforward_tx,
                            )
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

/// Handle an inbound bridge message: check the loop guard and duplicate
/// suppression, map the address, forward to the local hub, and re-forward to
/// other peers so messages propagate across multi-hop topologies.
async fn handle_inbound_message(
    msg: WireMessage,
    peer_addr: SocketAddr,
    loop_guard: &LoopGuard,
    engine: &MappingEngine,
    sink: &HubSink,
    reforward_tx: &broadcast::Sender<WireMessage>,
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

            // Duplicate suppression: each unique message is processed at most
            // once per bridge. Second sightings (the normal case in rings and
            // meshes) are dropped quietly.
            if let Some(msg_id) = protocol::get_msg_id(&msg) {
                if !loop_guard.check_and_record(&msg_id) {
                    return;
                }
            }

            // Re-forward to our other peers unchanged, preserving the
            // original origin and msg_id across hops. The broadcast also
            // echoes back down the session this message arrived on; the far
            // end's loop guard or duplicate suppression drops the echo.
            let _ = reforward_tx.send(msg.clone());

            // Map address and forward to local hub.
            let address = protocol::get_address(&msg).unwrap_or_default();
            let signal_type_str =
                protocol::get_signal_type(&msg).unwrap_or_else(|| "event".to_string());

            if let Some(mapped_address) =
                engine.map(&address, Direction::Inbound, Some(&signal_type_str))
            {
                let source = protocol::get_source(&msg);
                let payload = protocol::get_action_payload(&msg);

                // Parse signal type.
                let signal_type = match signal_type_str.as_str() {
                    "param" => SignalType::Param,
                    "stream" => SignalType::Stream,
                    _ => SignalType::Event,
                };

                // Bridged actions are forwarded as immediate (timestamp 0.0):
                // the wire timestamp is in the *sending* hub's clock domain
                // and the two hubs are not clock-synchronised, so honouring
                // it could schedule the action arbitrarily far in the local
                // future or past. True clock-domain rebasing is a deferred
                // enhancement.
                let action = action_with_source(source, &mapped_address, signal_type, 0.0, payload);

                eprintln!(
                    "[peer {}] Inbound: {} → {} (source={})",
                    peer_addr, address, mapped_address, source
                );

                if let Err(e) = sink.tx.send(action).await {
                    eprintln!("[peer {}] Failed to forward to hub: {}", peer_addr, e);
                }
            }
        }
        protocol::MSG_BRIDGE_UNSET => {
            // Check loop guard.
            if let Some(origin) = protocol::get_origin(&msg) {
                if loop_guard.is_loop(&origin) {
                    eprintln!(
                        "[peer {}] Dropping looped unset (origin={})",
                        peer_addr, origin
                    );
                    return;
                }
            }

            // Duplicate suppression, as for actions.
            if let Some(msg_id) = protocol::get_msg_id(&msg) {
                if !loop_guard.check_and_record(&msg_id) {
                    return;
                }
            }

            // Re-forward across hops, unchanged.
            let _ = reforward_tx.send(msg.clone());

            // Map the address and unset the param on the local hub. Unsets
            // are param semantics, so the signal filter sees "param".
            let address = protocol::get_address(&msg).unwrap_or_default();
            if let Some(mapped_address) = engine.map(&address, Direction::Inbound, Some("param")) {
                eprintln!(
                    "[peer {}] Inbound unset: {} → {}",
                    peer_addr, address, mapped_address
                );
                if let Err(e) = sink.tx.send(unset_param(mapped_address)).await {
                    eprintln!("[peer {}] Failed to forward unset to hub: {}", peer_addr, e);
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
