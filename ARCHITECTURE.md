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
- JPEG fallback via turbojpeg with persistent mmap
- DMA-heap buffer allocation for decoded frames

### image.rs

Hardware-accelerated image management using NXP G2D:

- DMA buffer allocation via dma-heap (CMA heap)
- G2D surface creation from Images and VPU Frames
- Color space conversion (NV12/YUYV to RGBA) via G2D blit
- Persistent memory mapping with mmap/munmap
- Physical address resolution via G2DPhysical

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

## Cache Coherency

DMA-buf buffers allocated from the CMA heap (`linux,cma`) are CPU-cached.
After G2D writes to these buffers via DMA, consumers must follow the complete
cache coherency protocol to avoid reading stale data:

1. **DRM PRIME import** — `DRM_IOCTL_PRIME_FD_TO_HANDLE` creates a persistent
   `dma_buf_attach`. Without this, `DMA_BUF_IOCTL_SYNC` is a no-op.
2. **Persistent mmap** — map once, keep for the buffer lifetime.
3. **SYNC_START** before CPU reads — invalidates CPU caches.
4. **SYNC_END** after CPU reads — completes the access.

See the [g2d-rs ARCHITECTURE.md](https://github.com/EdgeFirstAI/g2d-rs/blob/main/ARCHITECTURE.md)
for complete details.

**Current status:** The replay service does not implement DRM PRIME import.
Consumers reading published DMA-buf fds must handle cache coherency themselves,
or the system must use `linux,cma-uncached` heaps where no cache maintenance
is required.

## Dependencies

### Runtime

- **g2d-sys 1.2.0**: NXP G2D FFI bindings with automatic ABI version dispatch
- **videostream 2.1.4**: Video codec abstraction (V4L2 CODEC API)
- **zenoh 1.3.4**: Pub/sub messaging
- **mcap 0.18.0**: MCAP file format
- **turbojpeg 1.3.3**: JPEG decoding fallback
- **tokio 1.45.0**: Async runtime

### Hardware

- **G2D Library** (libg2d.so.2): NXP i.MX graphics acceleration
- **DMA-Heap**: Linux kernel DMA buffer allocation
- **VPU Driver**: Hardware video codec support
