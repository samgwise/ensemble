//! Local hub discovery — port file utilities.
//!
//! This module provides the fallback-first discovery mechanism used by Ensemble
//! clients to locate a running hub on the local machine. The hub writes a small
//! text file containing its bound TCP port to a well-known, platform-specific
//! location. Clients read that file before falling back to the default port.
//!
//! # Port file locations
//!
//! * Linux: `$XDG_RUNTIME_DIR/ensemble/hub.port`, falling back to
//!   `/tmp/ensemble-hub-{uid}.port` when `XDG_RUNTIME_DIR` is unavailable.
//! * macOS: `$TMPDIR/ensemble-hub.port`.
//! * Windows: `%LOCALAPPDATA%\Ensemble\hub.port`.
//!
//! The full path can be overridden at runtime by setting `ENSEMBLE_HUB_PORT_FILE`.
//! This is intended primarily for testing and sandboxed deployments.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Returns the platform-specific path to the hub port file.
///
/// If the `ENSEMBLE_HUB_PORT_FILE` environment variable is set, its value is
/// used verbatim. Otherwise the path is chosen according to the current
/// platform as documented in the module-level docs.
pub fn get_port_file_path() -> PathBuf {
    if let Ok(override_path) = env::var("ENSEMBLE_HUB_PORT_FILE") {
        return PathBuf::from(override_path);
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(runtime_dir).join("ensemble").join("hub.port")
        } else {
            PathBuf::from(format!("/tmp/ensemble-hub-{}.port", get_unix_uid()))
        }
    }

    #[cfg(target_os = "macos")]
    {
        let tmpdir = env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(tmpdir).join("ensemble-hub.port")
    }

    #[cfg(target_os = "windows")]
    {
        let local_app_data = env::var("LOCALAPPDATA").unwrap_or_else(|_| {
            env::var("USERPROFILE")
                .map(|p| format!("{}\\AppData\\Local", p))
                .unwrap_or_else(|_| ".".to_string())
        });
        PathBuf::from(local_app_data).join("Ensemble").join("hub.port")
    }
}

#[cfg(target_os = "linux")]
fn get_unix_uid() -> u32 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|content| {
            content.lines().find(|line| line.starts_with("Uid:")).and_then(|line| {
                line.split_whitespace()
                    .nth(1)
                    .and_then(|value| value.parse().ok())
            })
        })
        .unwrap_or(0)
}

/// Writes the hub port to the platform-specific port file.
///
/// The parent directory is created if it does not exist. On Unix, the file is
/// created with mode `0600` so it is readable only by the owning user.
pub fn write_port_file(port: u16) -> io::Result<()> {
    let path = get_port_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::File::create(&path)?;
    file.write_all(port.to_string().as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;

    #[cfg(unix)]
    {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&path, permissions)?;
    }

    Ok(())
}

/// Reads the hub port from the platform-specific port file.
///
/// Returns `None` if the file does not exist, is empty, or contains malformed
/// data. This allows callers to fall back to a default port without treating a
/// missing or stale port file as a fatal error.
pub fn read_port_file() -> Option<u16> {
    let path = get_port_file_path();
    let content = fs::read_to_string(&path).ok()?;
    content.trim().parse().ok()
}

/// Deletes the hub port file.
///
/// Returns `Ok(())` if the file was removed or did not exist. This makes it
/// safe to call during shutdown even if the file has already been cleaned up.
pub fn delete_port_file() -> io::Result<()> {
    let path = get_port_file_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Returns `true` if a process appears to be listening on the given port.
///
/// This is used for stale port file detection: a port file that points to a
/// port nobody is bound to is considered stale and can be removed safely.
///
/// The implementation attempts to bind to `127.0.0.1:port`. If the bind
/// succeeds, the port is free and the function returns `false`; if the bind
/// fails, the port is in use and the function returns `true`.
pub fn is_port_bound(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpListener::bind(addr).is_err()
}
