//! Local hub connection.
//!
//! Connects to the local Ensemble hub as an ordinary voice and handles
//! bidirectional forwarding between the hub and remote peers.

use anyhow::{Context, Result};
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::broadcast;

use crate::config::LocalConfig;
use crate::mapping::MappingEngine;

/// Connect to the local hub.
pub async fn connect_to_hub(config: &LocalConfig, name: &str) -> Result<Hub> {
    let hub = if let Some(port) = config.port {
        eprintln!("Connecting to local hub on port {}...", port);
        Hub::connect(port, name).await?
    } else {
        eprintln!("Discovering local hub...");
        Hub::connect_with_discovery(name).await?
    };
    
    eprintln!("Connected to local hub as voice #{}", hub.voice_id);
    Ok(hub)
}

/// Subscribe to outbound patterns on the local hub.
pub async fn subscribe_to_patterns(hub: &Hub, engine: &MappingEngine) -> Result<()> {
    let patterns = engine.outbound_patterns();
    for pattern in patterns {
        eprintln!("Subscribing to outbound pattern: {}", pattern.source());
        hub.subscribe(pattern.source()).await
            .context("failed to subscribe to pattern")?;
    }
    Ok(())
}

/// Forward an action from the local hub to remote peers.
pub async fn forward_to_remote(
    action: &WireMessage,
    engine: &MappingEngine,
    origin: &str,
    outbound_tx: &broadcast::Sender<WireMessage>,
) -> Result<()> {
    use crate::mapping::Direction;
    use crate::protocol::{bridge_action, get_address, get_signal_type, get_timestamp, get_source};
    
    let address = get_address(action).unwrap_or_default();
    let signal_type_str = get_signal_type(action).unwrap_or_else(|| "event".to_string());
    let timestamp = get_timestamp(action);
    let source = get_source(action);
    
    // Extract the payload from the action.
    let payload = match &action.payload {
        Value::Map(m) => m.get("payload").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    
    // Parse signal type.
    let signal_type = match signal_type_str.as_str() {
        "param" => SignalType::Param,
        "stream" => SignalType::Stream,
        _ => SignalType::Event,
    };
    
    // Map the address.
    if let Some(mapped_address) = engine.map(&address, Direction::Outbound, Some(&signal_type_str)) {
        let bridge_msg = bridge_action(
            origin,
            source,
            &mapped_address,
            signal_type,
            timestamp,
            payload,
        );
        
        // Broadcast to all peers (ignore error if no receivers).
        let _ = outbound_tx.send(bridge_msg);
    }
    
    Ok(())
}

/// Forward an action from a remote peer to the local hub.
#[allow(dead_code)]
pub async fn forward_to_local(
    bridge_msg: &WireMessage,
    engine: &MappingEngine,
    hub: &Hub,
) -> Result<()> {
    use crate::mapping::Direction;
    use crate::protocol::{get_address, get_signal_type, get_timestamp, get_source, get_action_payload};
    
    let address = get_address(bridge_msg).unwrap_or_default();
    let signal_type_str = get_signal_type(bridge_msg).unwrap_or_else(|| "event".to_string());
    let timestamp = get_timestamp(bridge_msg);
    let source = get_source(bridge_msg);
    let payload = get_action_payload(bridge_msg);
    
    // Parse signal type.
    let signal_type = match signal_type_str.as_str() {
        "param" => SignalType::Param,
        "stream" => SignalType::Stream,
        _ => SignalType::Event,
    };
    
    // Map the address (inbound direction).
    if let Some(mapped_address) = engine.map(&address, Direction::Inbound, Some(&signal_type_str)) {
        let action = action_with_source(
            source,
            &mapped_address,
            signal_type,
            timestamp,
            payload,
        );
        
        if let Err(e) = hub.send_action(action).await {
            eprintln!("Failed to forward action to local hub: {}", e);
        }
    }
    
    Ok(())
}