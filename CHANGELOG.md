# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.3.0] - 2026-05-22

### Changed

- **Breaking:** `rt/camera/dma` now publishes the decoder-native NV12 buffer,
  matching the live camera wire contract. Replay no longer performs NV12→RGBA
  conversion before publishing this topic. Downstream consumers that were
  reading RGBA from `rt/camera/dma` should switch to `--camera-image-topic`
  (sensor_msgs/Image, rgba8).
- Bumped `edgefirst-schemas` 1.3.1 → 3.4.0 and adopted its zero-copy
  view/builder API (`FoxgloveCompressedVideo::from_cdr`,
  `CompressedImage::from_cdr`, `Image::builder().encode_into_vec(..)`,
  `DmaBuffer::new(..).into_cdr()`).
- Upgraded `videostream` to 2.5.2. Switched the h264 decoder construction
  from the legacy `Decoder::create(H264, fps)` to
  `Decoder::create_ex(H264, fps, CodecBackend::Auto)`; the legacy entrypoint
  bypasses the v4l2 device enumeration the 2.5.x backend relies on, while
  the `_ex` entry uses it. On imx8mp this routes to `/dev/video1 vsi_v4l2dec`
  with a tighter natural NV12 stride (1920 bytes vs the Hantro path's 2880).
- `decode_h264_msg` retries on transient decoder backpressure (V4L2 m2m OUTPUT
  queue full): up to 20 attempts, 5 ms sleep between, retrying the same data.
  Mirrors `~/Software/EdgeFirst/camera/src/replay.rs`. Without this, the very
  first decode_frame call on a cold pool returns `Io("Decoder Error")` from
  the C library and is otherwise treated as fatal.
- `main` is now sync (no `#[tokio::main]`, no `tokio::time::sleep`, no
  `tokio::process::Command`). The edgefirst-hal 0.23.1 GL backend calls
  `tokio::sync::oneshot::Receiver::blocking_recv` during converter init,
  which panics inside a tokio runtime; making `main` sync sidesteps the
  collision. Service-handler invocations use `std::process::Command` and
  the inter-frame timing loop uses `std::thread::sleep`.

### Removed

- NXP G2D (`g2d-sys`), `turbojpeg`, `dma-heap`, `tokio`, and the per-frame
  `cdr` crate dependency. All image conversion, JPEG decoding, and CMA
  allocation now go through `edgefirst-hal` 0.23.1.
- `src/image.rs` (G2D surface helpers, CMA-heap `Image`, `MappedImage`).

### Added

- `--camera-image-topic` (env `CAMERA_IMAGE_TOPIC`) and `--camera-image-buffers`
  (env `CAMERA_IMAGE_BUFFERS`, default 4) — opt-in `sensor_msgs/Image` RGBA
  publisher for consumers that need a non-dma-buf-aware decoded image stream.
  Backed by `edgefirst-hal` `ImageProcessor` (auto-selects G2D / OpenGL / CPU).
  Source tensors cached by dma-buf inode; destination tensors pre-allocated
  once and reused.
- JPEG MCAP playback now uses the optimized `edgefirst-codec` JPEG decoder
  (`peek_info` + `ImageLoad::load_image`), writing directly into pre-allocated
  NV12 dma-buf tensors — no host-side intermediate, no memcpy.

## [2.2.0] - 2026-02-26

### Changed

- Renamed environment variables to short explicit names (MCAP, REPLAY_SPEED, TOPICS, etc.)
- Upload release binaries directly instead of zipping

### Added

- Complete `replay.default` configuration file for systemd EnvironmentFile usage
- `replay.default` published as a release artifact with install instructions

## [2.1.0] - 2026-02-16

### Changed

- Migrated from vendored g2d-sys 2.0.0 to upstream g2d-sys 1.2.0
- Release workflow uses wait-on-check pattern for cross-workflow artifact downloads
- Simplified rustfmt.toml to stable-only options (removed nightly requirements)
- Replaced per-frame mmap/munmap with persistent MappedImage mappings
- Used tokio::process::Command instead of std::process::Command in async context

### Added

- AI assistant development guidelines (.github/copilot-instructions.md)
- CI and EdgeFirst badges to README
- Cache coherency documentation in ARCHITECTURE.md
- Verify Builds sentinel job in build.yml for release coordination

### Removed

- Vendored g2d-sys sub-crate (replaced by upstream crate)
- Dead code: `Image::fd()` and `MappedImage::as_slice()` methods
- dma-buf dependency (no longer needed with upstream g2d-sys)

### Fixed

- CDR serialize error handling (replaced unwrap with match)
- munmap error check (changed `> 0` to `!= 0`)
- Tracy client handle lifetime (stored in variable to prevent premature drop)
- SBOM artifact naming in release workflow (handles *.cdx.json from cargo-cyclonedx)

## [2.0.0] - 2026-01-05

### Changed

- Migrated repository from Bitbucket to GitHub (EdgeFirstAI/replay)
- Changed license from proprietary EULA to Apache-2.0
- Upgraded videostream from 0.9.1 to 2.1.4 (V4L2 CODEC API support)
- Upgraded dma-buf from 0.4.0 to 0.5.0
- Updated g2d-sys license from AGPL-3.0 to Apache-2.0
- Renamed project from "Maivin Replay" to "EdgeFirst Replay"

### Added

- GitHub Actions CI/CD workflows (test, build, sbom, release)
- SBOM generation in CycloneDX format
- Comprehensive documentation (README, ARCHITECTURE, SECURITY, CONTRIBUTING)
- SPS v2.3 compliance
- SPDX license headers in all source files

## [1.2.2]

### Changed

- Changed image dimensions to use u32 instead of i32
- Updated dependencies
- Removed incorrect comments in g2d-sys

### Added

- Memory-mapped image support

## [1.2.1]

### Added

- System mode for replay service (`--system` flag)
- Looping mode flag (`--one-shot` to disable)
- Cargo config.toml for cross-linker configuration

### Changed

- Updated Bitbucket Pipelines and dependencies

## [1.2.0]

### Changed

- Ported to Zenoh 1.2
- Renamed setup.rs to args.rs
- Enabled SBOM generation with CycloneDX in CI/CD
- Updated dependencies with clippy and formatting fixes

### Added

- Tracing instrumentation

## [1.1.3]

### Fixed

- Unrecognized topics no longer refer to any service

### Added

- Lidar topic mapping to lidarpub service

## [1.1.2]

### Added

- Wildcard and keyexpr symbol support for topic selection
- Environment variable support for MCAP and replay speed arguments
- Handling for empty TOPICS="" case

### Fixed

- Cargo clippy warnings

## [1.1.1]

### Fixed

- Radar topic not mapping to radarpub service
- Disabled sonar audit until fixed

### Changed

- Updated to Rust 1.84.0

## [1.1.0]

### Added

- System service stopping when replaying (`--system` flag)
- Warning messages when errors occur stopping services

### Changed

- Updated dependency versions
- Each service only stopped once

## [1.0.1]

### Fixed

- `--list` flag not working
- Removed empty topic handling

## [1.0.0]

### Added

- Initial release
- MCAP file replay functionality
- H.264 and JPEG video decoding
- Zenoh pub/sub integration
- G2D hardware acceleration
- DMA buffer support
- Topic filtering
- Replay speed control
- Tracy profiler integration
