//! Ensemble Remote Bridge library.
//!
//! This crate provides the hub-to-hub bridge as both a library and a binary.
//! The library modules can be used for integration testing and embedding.

pub mod config;
pub mod local_hub;
pub mod loop_guard;
pub mod mapping;
pub mod param_cache;
pub mod peer_manager;
pub mod protocol;
pub mod remote_peer;
pub mod run;

pub use config::Config;
pub use run::{run_bridge, start_bridge, BridgeHandle};
