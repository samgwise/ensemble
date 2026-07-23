//! Integration tests for hub discovery — port file lifecycle.
//!
//! **Important**: These tests must be run with `--test-threads=1` because they
//! use `std::env::set_var` to override the port file path, which is process-global
//! state and not thread-safe. Running tests in parallel causes race conditions on
//! the environment variable.
//!
//! Example: `cargo test -p ensemble-hub --test discovery_integration -- --test-threads=1`

use std::fs;

use ensemble_discovery::{delete_port_file, is_port_bound, read_port_file, write_port_file};
use ensemble_hub::start_server;

/// Create a unique port file path for a test, stored under the OS temp dir.
fn temp_port_file(test_name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("ensemble-hub-discovery-tests");
    fs::create_dir_all(&dir).expect("create temp dir");
    dir.join(format!("{test_name}.port"))
}

/// Guard that sets `ENSEMBLE_HUB_PORT_FILE` for the duration of a test and
/// removes the file on drop.
struct PortFileEnv {
    path: std::path::PathBuf,
    _prev: Option<String>,
}

impl PortFileEnv {
    fn new(test_name: &str) -> Self {
        let path = temp_port_file(test_name);
        let prev = std::env::var("ENSEMBLE_HUB_PORT_FILE").ok();
        // SAFETY: tests run with --test-threads=1 or use unique paths.
        std::env::set_var("ENSEMBLE_HUB_PORT_FILE", &path);
        Self { path, _prev: prev }
    }
}

impl Drop for PortFileEnv {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Some(ref prev) = self._prev {
            std::env::set_var("ENSEMBLE_HUB_PORT_FILE", prev);
        } else {
            std::env::remove_var("ENSEMBLE_HUB_PORT_FILE");
        }
    }
}

#[tokio::test]
async fn hub_startup_creates_port_file_with_correct_content() {
    let _env = PortFileEnv::new("startup-creates");

    let (_state, actual_port) = start_server(0).await.expect("start server");
    write_port_file(actual_port).expect("write port file");

    let content = fs::read_to_string(_env.path.clone()).expect("read port file");
    let port_in_file: u16 = content.trim().parse().expect("parse port");
    assert_eq!(port_in_file, actual_port);

    // Clean up.
    delete_port_file().ok();
}

#[tokio::test]
async fn port_file_contains_actual_bound_port() {
    let _env = PortFileEnv::new("actual-bound-port");

    // Request port 0 so the OS assigns a free port.
    let (_state, actual_port) = start_server(0).await.expect("start server");
    assert_ne!(actual_port, 0, "OS should assign a non-zero port");

    write_port_file(actual_port).expect("write port file");

    let discovered = read_port_file().expect("port file should exist");
    assert_eq!(discovered, actual_port);

    // The port should be bound (our server is still running).
    assert!(is_port_bound(actual_port));

    delete_port_file().ok();
}

#[tokio::test]
async fn stale_port_file_cleanup_on_startup() {
    let _env = PortFileEnv::new("stale-cleanup");

    // Find a port that is NOT bound by writing a port file with a port
    // we know is free.
    let stale_port = {
        // Bind to port 0 to get a free port, then drop the listener.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };

    // Confirm the port is not bound.
    assert!(!is_port_bound(stale_port));

    // Write a stale port file pointing to the unbound port.
    write_port_file(stale_port).expect("write stale port file");
    assert_eq!(read_port_file(), Some(stale_port));

    // Simulate the stale cleanup logic from main.rs.
    if let Some(port) = read_port_file() {
        if !is_port_bound(port) {
            delete_port_file().ok();
        }
    }

    // The stale port file should have been removed.
    assert_eq!(
        read_port_file(),
        None,
        "stale port file should be cleaned up"
    );
}
