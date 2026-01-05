# Architecture

## Overview

EdgeFirst Replay is a Rust application that replays MCAP recordings through
the Zenoh messaging system, with special handling for video streams using
hardware-accelerated decoding and DMA buffer sharing.

## Component Diagram

```
+----------------+     +------------------+     +----------------+
|   MCAP File    | --> |  Message Stream  | --> |    Zenoh       |
|   (mcap crate) |     |    Processing    |     |   Publisher    |
+----------------+     +------------------+     +----------------+
                              |
                              v
                    +------------------+
                    |  Video Pipeline  |
                    +------------------+
                    |  H.264 Decoder   |
                    |  (videostream)   |
                    +--------+---------+
                             |
                    +--------v---------+
                    |   G2D Converter  |
                    |   (g2d-sys FFI)  |
                    +--------+---------+
                             |
                    +--------v---------+
                    |   DMA Buffer     |
                    |   Publishing     |
                    +------------------+
```

## Modules

### main.rs

Entry point and core replay logic:

- MCAP file memory-mapping and parsing
- Message filtering by topic patterns
- Zenoh session management and message publishing
- Timing control for replay speed
- Tracy profiler integration

### video_decode.rs

H.264 and JPEG decoding with frame buffer management:

- Hardware VPU decoder via videostream library
- Frame buffer pool (4 buffers for pipelining)
- JPEG fallback via turbojpeg
- G2D buffer allocation for decoded frames

### image.rs

G2D graphics library wrapper for hardware-accelerated operations:

- DMA buffer allocation via dma-heap
- Color space conversion (NV12/YUYV to RGBA)
- Memory mapping with mmap/munmap
- Physical address handling for zero-copy
- G2D version detection (supports 6.4.3+)

### args.rs

CLI argument parsing with Zenoh configuration:

- Clap-based argument definition
- Environment variable support
- Topic pattern parsing
- Zenoh configuration generation

### services.rs

System service control for topic conflict resolution:

- Topic to service mapping (camera, radar, IMU, GPS, Lidar)
- Async service stopping via systemctl
- Conflict detection and resolution

#### services.json Configuration

The embedded `services.json` file maps MCAP topic prefixes to systemd service names.
When replay runs with `--system` flag, it stops services that would conflict with
replayed topics.

```json
{
    "camera": "camera",      // /camera/* topics -> camera.service
    "radar": "radarpub",     // /radar/* topics -> radarpub.service
    "imu": "imu",            // /imu/* topics -> imu.service
    "gps": "navsat",         // /gps/* topics -> navsat.service
    "lidar": "lidarpub",     // /lidar/* topics -> lidarpub.service
    "tf_static": "NONE"      // Static transforms - no service to stop
}
```

Topics not matching any prefix are ignored (no service stopped).

### g2d-sys (sub-crate)

FFI bindings for NXP G2D graphics library:

- Safe wrapper around G2D C library
- Format conversions (FourCC to G2D format)
- Physical address handling
- Frame conversion utilities
- Version compatibility (handles G2D 2.3.0 API changes)

## Data Flow

1. **MCAP Parsing**: File is memory-mapped and parsed using the mcap crate
2. **Topic Filtering**: Messages filtered by include/exclude patterns
3. **Video Detection**: H.264 and JPEG streams identified by topic/encoding
4. **Hardware Decoding**: Video frames decoded via VPU (videostream library)
5. **Color Conversion**: Decoded frames converted to RGBA via G2D hardware
6. **DMA Publishing**: Frames published as DMA buffers on Zenoh topics
7. **Passthrough**: Non-video messages published unchanged

## Performance Considerations

### Zero-Copy Pipeline

The system uses DMA buffers throughout to minimize memory copies:

- MCAP file is memory-mapped (not loaded into RAM)
- Decoder outputs directly to DMA buffers
- G2D operates on physical addresses
- Zenoh publishes DMA buffer file descriptors

### Frame Buffer Pool

A pool of 4 frame buffers enables pipelining:

- Decoder can work on frame N while G2D processes frame N-1
- Reduces latency compared to single-buffer approach
- Round-robin allocation prevents memory fragmentation

### Hardware Acceleration

- **VPU**: Hardware video decoding (H.264)
- **G2D**: Hardware 2D graphics operations
- **DMA-Heap**: Kernel-managed DMA buffer allocation

## Dependencies

### Runtime

- **videostream 2.1.4**: Video codec abstraction (V4L2 CODEC API)
- **zenoh 1.3.4**: Pub/sub messaging
- **mcap 0.18.0**: MCAP file format
- **turbojpeg 1.3.3**: JPEG decoding fallback
- **tokio 1.45.0**: Async runtime

### Hardware

- **G2D Library** (libg2d.so.2): NXP i.MX graphics acceleration
- **DMA-Heap**: Linux kernel DMA buffer allocation
- **VPU Driver**: Hardware video codec support
