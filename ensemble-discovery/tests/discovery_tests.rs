//! Tests for the local hub discovery port file utilities.
//!
//! Tests that touch environment variables acquire a global mutex because
//! environment variables are process-global state and Rust tests run in parallel
//! by default.

use ensemble_discovery as discovery;
use std::env;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Mutex;

// Serialise any test that mutates environment variables.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_port_file(name: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "ensemble-discovery-test-{}-{}.port",
        name,
        std::process::id()
    ))
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn with_override<F>(path: PathBuf, f: F)
where
    F: FnOnce(PathBuf),
{
    let _guard = ENV_LOCK.lock().unwrap();
    let _ = fs::remove_file(&path);
    env::set_var("ENSEMBLE_HUB_PORT_FILE", &path);
    f(path.clone());
    env::remove_var("ENSEMBLE_HUB_PORT_FILE");
    let _ = fs::remove_file(&path);
}

#[test]
fn port_file_path_override() {
    with_override(temp_port_file("override"), |path| {
        assert_eq!(discovery::get_port_file_path(), path);
    });
}

#[cfg(target_os = "windows")]
#[test]
fn port_file_path_windows() {
    let _guard = ENV_LOCK.lock().unwrap();
    env::remove_var("ENSEMBLE_HUB_PORT_FILE");
    env::set_var("LOCALAPPDATA", r"C:\Users\Test\AppData\Local");

    let path = discovery::get_port_file_path();
    assert_eq!(
        path,
        PathBuf::from(r"C:\Users\Test\AppData\Local\Ensemble\hub.port")
    );

    env::remove_var("LOCALAPPDATA");
}

#[cfg(target_os = "macos")]
#[test]
fn port_file_path_macos() {
    let _guard = ENV_LOCK.lock().unwrap();
    env::remove_var("ENSEMBLE_HUB_PORT_FILE");
    env::set_var("TMPDIR", "/var/folders/test/");

    let path = discovery::get_port_file_path();
    assert_eq!(path, PathBuf::from("/var/folders/test/ensemble-hub.port"));

    env::remove_var("TMPDIR");
}

#[cfg(target_os = "linux")]
#[test]
fn port_file_path_linux_xdg() {
    let _guard = ENV_LOCK.lock().unwrap();
    env::remove_var("ENSEMBLE_HUB_PORT_FILE");
    env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");

    let path = discovery::get_port_file_path();
    assert_eq!(path, PathBuf::from("/run/user/1000/ensemble/hub.port"));

    env::remove_var("XDG_RUNTIME_DIR");
}

#[cfg(target_os = "linux")]
#[test]
fn port_file_path_linux_fallback() {
    let _guard = ENV_LOCK.lock().unwrap();
    env::remove_var("ENSEMBLE_HUB_PORT_FILE");
    env::remove_var("XDG_RUNTIME_DIR");

    let path = discovery::get_port_file_path();
    let path_str = path.to_string_lossy();
    assert!(
        path_str.starts_with("/tmp/ensemble-hub-") && path_str.ends_with(".port"),
        "expected /tmp/ensemble-hub-<uid>.port, got {}",
        path_str
    );
}

#[test]
fn write_read_delete_cycle() {
    with_override(temp_port_file("cycle"), |path| {
        discovery::write_port_file(7331).unwrap();
        assert!(path.exists());
        assert_eq!(discovery::read_port_file(), Some(7331));

        discovery::delete_port_file().unwrap();
        assert!(!path.exists());

        // Deleting a missing port file must not be an error.
        discovery::delete_port_file().unwrap();
    });
}

#[test]
fn stale_port_file_detection() {
    with_override(temp_port_file("stale"), |_| {
        let free_port = find_free_port();
        discovery::write_port_file(free_port).unwrap();

        assert_eq!(discovery::read_port_file(), Some(free_port));
        assert!(
            !discovery::is_port_bound(free_port),
            "a free port should not be reported as bound"
        );
    });
}

#[test]
fn malformed_port_file() {
    with_override(temp_port_file("malformed"), |path| {
        fs::write(&path, "not-a-number\n").unwrap();
        assert_eq!(discovery::read_port_file(), None);

        fs::write(&path, "").unwrap();
        assert_eq!(discovery::read_port_file(), None);

        fs::write(&path, "7331\nextra\n").unwrap();
        assert_eq!(discovery::read_port_file(), None);
    });
}

#[test]
fn is_port_bound_with_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(
        discovery::is_port_bound(port),
        "a bound port should be reported as bound"
    );

    drop(listener);

    let free_port = find_free_port();
    assert!(
        !discovery::is_port_bound(free_port),
        "a free port should not be reported as bound"
    );
}
