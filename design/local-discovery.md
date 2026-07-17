# Ensemble Local Hub Discovery Specification (Draft v0.1)

## Status

Draft v0.1

## Purpose

This specification defines how Ensemble clients discover and connect to a local hub.

Ensemble is designed for local-first operation, where clients and the hub typically run on the same machine or within the same local network.

This document addresses:

- default port conventions
- platform-appropriate discovery mechanisms
- port file format and location
- override strategies

## Design Principles

### Local-First

Ensemble assumes clients and hub share a trusted local environment.

Discovery mechanisms are optimised for:

- low latency
- zero configuration
- minimal dependencies

### Platform Pragmatism

Different platforms have different capabilities for inter-process communication and service discovery.

The specification defines a *discovery strategy* rather than mandating a single mechanism.

### Fallback-First

A well-known default port provides a universal fallback that works in all environments.

Discovery mechanisms are enhancements, not requirements.

## Default Port

The default TCP port for Ensemble hub connections is:

```text
7331
```

This port SHOULD be used when:

- no discovery mechanism succeeds
- the user has not specified an alternative
- the environment lacks platform-appropriate discovery

Implementations MAY allow the default port to be overridden through configuration.

## Discovery Strategy

Clients SHOULD attempt discovery in the following order:

1. **Explicit configuration**: User-specified port via CLI argument, environment variable, or configuration file
2. **Platform-appropriate discovery**: Native service discovery mechanisms (see below)
3. **Default port fallback**: Connect to the well-known default port

If all discovery attempts fail, the client SHOULD report an error and terminate.

## Port File Discovery (Desktop Platforms)

On desktop platforms (Linux, macOS, Windows), the hub SHOULD write a port file upon successful binding.

### Port File Format

The port file contains a single line:

```text
{port}
```

Where `{port}` is the TCP port number the hub is listening on.

Example:

```text
7331
```

Or, if the hub bound to a different port:

```text
54321
```

### Port File Location

The port file location is platform-specific:

**Linux:**

```text
$XDG_RUNTIME_DIR/ensemble/hub.port
```

Falling back to:

```text
/tmp/ensemble-hub-{uid}.port
```

Where `{uid}` is the user's numeric UID.

**macOS:**

```text
$TMPDIR/ensemble-hub.port
```

Typically resolves to:

```text
/var/folders/.../T/ensemble-hub.port
```

**Windows:**

```text
%LOCALAPPDATA%\Ensemble\hub.port
```

Typically resolves to:

```text
C:\Users\{username}\AppData\Local\Ensemble\hub.port
```

### Port File Lifecycle

The hub MUST:

- Create the port file after successful binding
- Delete the port file on graceful shutdown
- Handle stale port files from crashed hubs (see below)

Clients SHOULD:

- Read the port file if it exists
- Verify the port is reachable before attempting connection
- Fall back to the default port if the file is missing or unreadable

### Stale Port File Handling

If the hub crashes or is killed without cleanup, the port file may remain.

Clients SHOULD:

- Attempt to connect to the port specified in the file
- If the connection fails, delete the stale file and fall back to the default port

Hubs SHOULD:

- Check for existing port files on startup
- Verify the port is in use before overwriting
- Overwrite stale files if the port is not bound

## Mobile and Embedded Platforms

On platforms with restricted filesystem access (Android, iOS, embedded), the port file mechanism is not applicable.

Implementations SHOULD use platform-native discovery:

**Android:**

- Use Android Services for inter-process communication
- Use ContentProviders to share connection information between apps from the same developer
- Use local broadcasts for same-process discovery

**iOS:**

- Use Bonjour/mDNS for local network discovery
- Use XPC for inter-process communication (same developer)
- Use App Groups for shared container access (same developer)

**Embedded:**

- Use platform-specific IPC mechanisms
- Use well-known ports if discovery is unavailable

If native discovery is unavailable or fails, implementations SHOULD fall back to the default port.

## Override Mechanisms

Implementations SHOULD support multiple ways to override the default port:

### Environment Variable

The hub and clients SHOULD check for:

```text
ENSEMBLE_HUB_PORT
```

If set, this value overrides the default port.

Example:

```bash
export ENSEMBLE_HUB_PORT=8000
ensemble-hub
```

### Command-Line Argument

The hub SHOULD accept a port argument:

```bash
ensemble-hub --port 8000
```

Clients MAY accept a port argument:

```bash
ensemble-client --hub-port 8000
```

### Configuration File

Implementations MAY support a configuration file at:

**Linux/macOS:**

```text
$XDG_CONFIG_HOME/ensemble/config.toml
```

**Windows:**

```text
%APPDATA%\Ensemble\config.toml
```

Example configuration:

```toml
[hub]
port = 8000
```

### Priority Order

When multiple override mechanisms are present, the priority order SHOULD be:

1. Command-line argument (highest)
2. Environment variable
3. Configuration file
4. Discovery mechanism
5. Default port (lowest)

## Security Considerations

The port file and discovery mechanisms operate within a trusted local environment.

Implementations SHOULD:

- Use file permissions to restrict port file access to the current user
- Validate port numbers (1-65535)
- Handle malformed port files gracefully

Implementations MUST NOT:

- Expose the port file to other users on multi-user systems
- Trust port files from untrusted sources

## Future Extensions

This specification may be extended to support:

- **Unix domain sockets**: Alternative transport for local-only communication
- **mDNS/Bonjour**: Network-wide discovery for multi-machine setups
- **Hub clustering**: Discovery of multiple hubs for distributed operation

These extensions are outside the scope of v0.1.

## Summary

Local hub discovery in Ensemble follows a pragmatic, fallback-first strategy:

- Default port `7331` provides universal fallback
- Port file mechanism for desktop platforms
- Platform-native discovery for mobile/embedded
- Multiple override mechanisms for flexibility

This approach balances simplicity, cross-platform compatibility, and user control.
