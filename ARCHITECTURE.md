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
                    |  H.264 (V4L2)    |
                    |  JPEG (hal codec)|
                    +--------+---------+
                             |
                    +--------v---------+
                    |  CameraFrame     |
                    |  (NV12 dma-buf)  |
                    +--------+---------+
                             |
                    +--------v---------+
                    |  Optional RGBA   |
                    |  (hal ImageProc) |
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

H.264 and JPEG decoding:

- Hardware V4L2 mem2mem decoder via videostream (`Decoder::create_ex`)
- Transient OUTPUT-queue backpressure retries
- JPEG decode through `edgefirst-codec` into a pre-allocated NV12 dma-buf ring

### image_publish.rs

Optional `sensor_msgs/Image` RGBA side channel (`--camera-image-topic`):

- `edgefirst-hal` `ImageProcessor` (G2D / OpenGL / CPU)
- Inode-keyed source tensor cache and a reused destination ring

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
4. **Hardware Decoding**: H.264 via videostream V4L2; JPEG via edgefirst-codec
5. **CameraFrame Publishing**: Decoder-native NV12 dma-buf as schemas 4.0
   `CameraFrame` on `--dma-topic` (default `rt/camera/frame`)
6. **Optional RGBA**: `--camera-image-topic` converts NV12 → `sensor_msgs/Image`
7. **Passthrough**: Non-video messages published unchanged. Recorded
   `DmaBuffer` / `CameraFrame` messages are skipped (fds are process-local).

## Performance Considerations

### Zero-Copy Pipeline

The system uses DMA buffers throughout to minimize memory copies:

- MCAP file is memory-mapped (not loaded into RAM)
- Decoder outputs directly to DMA buffers
- JPEG decode writes into a pre-allocated NV12 dma-buf ring
- Zenoh publishes CameraFrame descriptors that name those fds

### Hardware Acceleration

- **VPU / V4L2**: Hardware H.264 decode (`/dev/video1 vsi_v4l2dec` on imx8mp)
- **HAL ImageProcessor**: Optional NV12 → RGBA (G2D / OpenGL / CPU)
- **edgefirst-codec**: JPEG decode into dma-buf tensors

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

- **edgefirst-hal / edgefirst-codec 0.23.1**: Image conversion and JPEG decode
- **edgefirst-schemas 4.0**: Zero-copy `CameraFrame` / compressed-video views
- **videostream 2.5.3**: V4L2 H.264 decode
- **zenoh 1.3.4**: Pub/sub messaging
- **mcap 0.18.0**: MCAP file format

### Hardware

- **V4L2 decoder** (`vsidaemon` + `/dev/video1` on imx8mp)
- **Optional G2D / GPU** via HAL for `--camera-image-topic`
