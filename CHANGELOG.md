# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
