//! Local hub discovery — port file utilities.
//!
//! This crate provides the fallback-first discovery mechanism used by Ensemble
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
//! # Runtime override
//!
//! The full path can be overridden at runtime in two ways:
//!
//! 1. **Environment variable**: Set `ENSEMBLE_HUB_PORT_FILE` to a custom path.
//!    This is intended for testing and sandboxed deployments.
//! 2. **Programmatic override**: Call [`set_port_file_path()`] to override the
//!    path for the current process. This takes precedence over the environment
//!    variable and is useful for custom deployments or embedded scenarios.
//!
//! # Platform abstraction
//!
//! This crate is designed to be lightweight and portable. It contains no
//! external dependencies and relies only on the Rust standard library.
//!
//! For mobile platforms (Android, iOS) or embedded systems that require
//! different discovery mechanisms, this crate can be replaced with a
//! platform-specific implementation that provides the same public API.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Global override for the port file path.
///
/// When set, this takes precedence over the `ENSEMBLE_HUB_PORT_FILE` environment
/// variable and the platform-specific default paths.
static PORT_FILE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets a custom path for the hub port file.
///
/// This override takes precedence over the `ENSEMBLE_HUB_PORT_FILE` environment
/// variable and the platform-specific default paths. It is useful for:
///
/// - Testing (avoiding conflicts with real hub instances)
/// - Custom deployments (non-standard installation locations)
/// - Embedded systems (custom storage locations)
/// - Sandboxed environments (restricted filesystem access)
///
/// Pass `None` to clear the override and revert to the default behavior.
///
/// # Example
///
/// ```no_run
/// use ensemble_discovery::set_port_file_path;
/// use std::path::PathBuf;
///
/// // Set a custom path
/// set_port_file_path(Some(PathBuf::from("/tmp/my-custom-hub.port")));
///
/// // Clear the override
/// set_port_file_path(None);
/// ```
pub fn set_port_file_path(path: Option<PathBuf>) {
    let mut override_path = PORT_FILE_OVERRIDE.lock().unwrap();
    *override_path = path;
}

/// Returns the platform-specific path to the hub port file.
///
/// The path is determined in the following priority order:
///
/// 1. **Programmatic override**: If [`set_port_file_path()`] was called with
///    `Some(path)`, that path is returned.
/// 2. **Environment variable**: If `ENSEMBLE_HUB_PORT_FILE` is set, its value
///    is used verbatim.
/// 3. **Platform default**: Otherwise, the platform-specific default path is
///    returned (see crate-level documentation).
pub fn get_port_file_path() -> PathBuf {
    // Check programmatic override first
    {
        let override_path = PORT_FILE_OVERRIDE.lock().unwrap();
        if let Some(ref path) = *override_path {
            return path.clone();
        }
    }

    // Check environment variable
    if let Ok(override_path) = env::var("ENSEMBLE_HUB_PORT_FILE") {
        return PathBuf::from(override_path);
    }

    // Fall back to platform-specific default
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
///
/// # Arguments
///
/// * `port` - The TCP port number the hub is listening on
///
/// # Returns
///
/// * `Ok(())` if the port file was successfully written
/// * `Err(io::Error)` if the file could not be created or written
///
/// # Example
///
/// ```no_run
/// use ensemble_discovery::write_port_file;
///
/// write_port_file(7331).expect("Failed to write port file");
/// ```
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
///
/// # Returns
///
/// * `Some(port)` if the port file exists and contains a valid port number
/// * `None` if the file is missing, empty, or contains invalid data
///
/// # Example
///
/// ```no_run
/// use ensemble_discovery::read_port_file;
///
/// if let Some(port) = read_port_file() {
///     println!("Hub is running on port {}", port);
/// } else {
///     println!("No hub port file found, using default port");
/// }
/// ```
pub fn read_port_file() -> Option<u16> {
    let path = get_port_file_path();
    let content = fs::read_to_string(&path).ok()?;
    content.trim().parse().ok()
}

/// Deletes the hub port file.
///
/// Returns `Ok(())` if the file was removed or did not exist. This makes it
/// safe to call during shutdown even if the file has already been cleaned up.
///
/// # Returns
///
/// * `Ok(())` if the file was successfully deleted or did not exist
/// * `Err(io::Error)` if the file exists but could not be deleted
///
/// # Example
///
/// ```no_run
/// use ensemble_discovery::delete_port_file;
///
/// delete_port_file().expect("Failed to delete port file");
/// ```
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
///
/// # Arguments
///
/// * `port` - The TCP port number to check
///
/// # Returns
///
/// * `true` if the port appears to be in use
/// * `false` if the port is free
///
/// # Example
///
/// ```no_run
/// use ensemble_discovery::is_port_bound;
///
/// if is_port_bound(7331) {
///     println!("Port 7331 is in use");
/// } else {
///     println!("Port 7331 is free");
/// }
/// ```
pub fn is_port_bound(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpListener::bind(addr).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    // Serialize tests that modify the global port file override
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_set_and_clear_override() {
        let _guard = TEST_LOCK.lock().unwrap();
        let custom_path = env::temp_dir().join("test-override.port");

        // Set override
        set_port_file_path(Some(custom_path.clone()));
        assert_eq!(get_port_file_path(), custom_path);

        // Clear override
        set_port_file_path(None);
        // Should now return platform default or env var value
        assert_ne!(get_port_file_path(), custom_path);
    }

    #[test]
    fn test_override_takes_precedence_over_env() {
        let _guard = TEST_LOCK.lock().unwrap();
        let custom_path = env::temp_dir().join("test-precedence.port");
        let env_path = env::temp_dir().join("test-env.port");

        // Set env var
        env::set_var("ENSEMBLE_HUB_PORT_FILE", env_path.to_str().unwrap());

        // Set override
        set_port_file_path(Some(custom_path.clone()));

        // Override should take precedence
        assert_eq!(get_port_file_path(), custom_path);

        // Clear override
        set_port_file_path(None);

        // Now env var should be used
        assert_eq!(get_port_file_path(), env_path);

        // Clean up
        env::remove_var("ENSEMBLE_HUB_PORT_FILE");
    }

    #[test]
    fn test_write_and_read_port_file() {
        let _guard = TEST_LOCK.lock().unwrap();
        let temp_path = env::temp_dir().join("test-write-read.port");
        set_port_file_path(Some(temp_path.clone()));

        // Write port
        write_port_file(7331).expect("Failed to write port file");

        // Read port
        let port = read_port_file();
        assert_eq!(port, Some(7331));

        // Clean up
        delete_port_file().ok();
        set_port_file_path(None);
        fs::remove_file(&temp_path).ok();
    }

    #[test]
    fn test_delete_nonexistent_port_file() {
        let _guard = TEST_LOCK.lock().unwrap();
        let temp_path = env::temp_dir().join("test-nonexistent.port");
        set_port_file_path(Some(temp_path));

        // Should not error even if file doesn't exist
        let result = delete_port_file();
        assert!(result.is_ok());

        set_port_file_path(None);
    }

    #[test]
    fn test_read_malformed_port_file() {
        let _guard = TEST_LOCK.lock().unwrap();
        let temp_path = env::temp_dir().join("test-malformed.port");
        set_port_file_path(Some(temp_path.clone()));

        // Write invalid content
        fs::write(&temp_path, "not-a-number\n").expect("Failed to write test file");

        // Should return None
        let port = read_port_file();
        assert_eq!(port, None);

        // Clean up
        fs::remove_file(&temp_path).ok();
        set_port_file_path(None);
    }

    #[test]
    fn test_is_port_bound() {
        // Port 0 should never be bound (OS assigns a free port)
        assert!(!is_port_bound(0));

        // Bind to a port and check
        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let port = listener.local_addr().unwrap().port();

        // Now it should be bound
        assert!(is_port_bound(port));

        // Drop listener
        drop(listener);

        // Now it should be free again
        assert!(!is_port_bound(port));
    }
}
