# ensemble-bridge-osc

OSC bridge for Ensemble — translates between Ensemble actions and OSC/UDP.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```bash
# Default: listen for OSC on UDP 9001, send OSC to 127.0.0.1:9000
ensemble-bridge-osc

# Custom configuration (e.g. SuperCollider)
ensemble-bridge-osc --name sc-bridge --osc-send-port 57120 --osc-listen-port 57121
```

Ensemble actions under the `--ens-prefix` address prefix (default `/osc/out`)
are forwarded as OSC messages, and received OSC messages are published back as
Ensemble actions under `/osc/in`. Only `action` messages are translated — hub
`unset_param` notifications are ignored.

| Option | Default | Description |
|--------|---------|-------------|
| `--name` | `osc-bridge` | Voice name shown in the hub |
| `--ens-prefix` | `/osc/out` | Ensemble prefix for outbound actions |
| `--osc-prefix` | (empty) | OSC prefix for address mapping |
| `--osc-send-host` | `127.0.0.1` | Host to send OSC messages to |
| `--osc-send-port` | `9000` | Port to send OSC messages to |
| `--osc-listen-port` | `9001` | UDP port to listen for inbound OSC |
| `--hub` | (discovery) | Explicit hub port |
