# EdgeFirst Replay

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

MCAP replay utility for EdgeFirst edge AI platforms. Replays recorded sensor
data through Zenoh messaging for development, testing, and demonstration.

## Features

- MCAP file playback with timing preservation
- H.264 and JPEG video decoding via hardware VPU
- DMA buffer sharing for zero-copy video pipelines
- Zenoh pub/sub integration for message distribution
- Topic filtering (include/exclude patterns)
- Configurable replay speed
- System service control (can stop camera, radar, IMU, GPS, Lidar services)
- Tracy profiler integration for performance analysis

## Requirements

- Linux (aarch64 or x86_64)
- G2D library (libg2d.so.2) for hardware-accelerated graphics
- DMA-Heap support for buffer allocation
- Rust 1.70 or later (for building from source)

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/EdgeFirstAI/replay.git
cd replay

# Build release binary
cargo build --release

# Binary will be at target/release/edgefirst-replay
```

### Cross-compilation for ARM64

```bash
cargo build --release --target aarch64-unknown-linux-gnu
```

## Usage

```bash
edgefirst-replay <MCAP_FILE> [OPTIONS]
```

### Examples

```bash
# Basic replay at normal speed
edgefirst-replay recording.mcap

# Replay at 2x speed
edgefirst-replay recording.mcap --replay-speed 2.0

# List all topics in the recording
edgefirst-replay recording.mcap --list

# Replay only camera topics
edgefirst-replay recording.mcap --topics "/camera/**"

# Replay once without looping
edgefirst-replay recording.mcap --one-shot

# Stop conflicting system services before replay
edgefirst-replay recording.mcap --system
```

### Options

| Option | Description | Default |
|--------|-------------|---------|
| `-r, --replay-speed` | Playback speed multiplier | `1.0` |
| `-l, --list` | List topics in MCAP file | - |
| `-o, --one-shot` | Play once without looping | - |
| `-s, --system` | Stop conflicting system services | - |
| `-t, --topics` | Topics to publish (space-separated) | All topics |
| `-i, --ignore-topics` | Topics to ignore | - |
| `--dma-topic` | Raw DMA buffer topic | `rt/camera/dma` |
| `--rust-log` | Application log level | `info` |
| `--tracy` | Enable Tracy profiler broadcast | - |
| `--mode` | Zenoh connection mode | `peer` |
| `--connect` | Zenoh endpoints to connect to | - |
| `--listen` | Zenoh endpoints to listen on | - |
| `--no-multicast-scouting` | Disable Zenoh multicast discovery | - |

### Environment Variables

All options can be set via environment variables:

- `MCAP` - Path to MCAP file
- `REPLAY_SPEED` - Playback speed
- `TOPICS` - Topics to publish (space-separated)
- `IGNORE_TOPICS` - Topics to ignore
- `RUST_LOG` - Log level
- `TRACY` - Enable Tracy profiler

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for system design details.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines.

## License

Apache-2.0 - See [LICENSE](LICENSE) for details.

## Security

See [SECURITY.md](SECURITY.md) for vulnerability reporting.
