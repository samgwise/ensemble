# ensemble-discovery

Local hub discovery for Ensemble — port file utilities.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_discovery::{read_port_file, write_port_file};

write_port_file(7331)?;       // hub side, after binding
let port = read_port_file();  // client side: Option<u16>
```

`write_port_file` writes atomically (staging file + rename), so readers never observe a partially written port file.
