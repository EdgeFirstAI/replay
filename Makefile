# EdgeFirst Replay Makefile
# SPDX-License-Identifier: Apache-2.0
# Copyright 2025 Au-Zone Technologies Inc.

.PHONY: all build release test lint fmt clean help verify-version pre-release

# Default target
all: build

# Build targets
build:
	cargo build

release:
	cargo build --release

# Cross-compilation for ARM64
build-aarch64:
	cargo build --release --target aarch64-unknown-linux-gnu

# Testing
test:
	cargo test

coverage:
	cargo llvm-cov --html

# Code quality
lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# Documentation
doc:
	cargo doc --no-deps

# Cleaning
clean:
	cargo clean

# Version verification
verify-version:
	@echo "Checking version consistency..."
	@VERSION=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/'); \
	G2D_VERSION=$$(grep '^version' g2d-sys/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/'); \
	echo "Main version: $$VERSION"; \
	echo "g2d-sys version: $$G2D_VERSION"; \
	if [ "$$VERSION" != "$$G2D_VERSION" ]; then \
		echo "ERROR: Version mismatch between Cargo.toml and g2d-sys/Cargo.toml"; \
		exit 1; \
	fi
	@echo "Version check passed!"

# Pre-release validation
pre-release: clean fmt-check lint test verify-version
	@echo "Pre-release checks passed!"

# Help
help:
	@echo "EdgeFirst Replay - Available targets:"
	@echo ""
	@echo "  build          - Build debug binary"
	@echo "  release        - Build release binary"
	@echo "  build-aarch64  - Cross-compile for ARM64"
	@echo "  test           - Run tests"
	@echo "  coverage       - Generate coverage report (requires cargo-llvm-cov)"
	@echo "  lint           - Run clippy linter"
	@echo "  fmt            - Format code"
	@echo "  fmt-check      - Check code formatting"
	@echo "  doc            - Generate documentation"
	@echo "  clean          - Remove build artifacts"
	@echo "  verify-version - Check version consistency"
	@echo "  pre-release    - Run all pre-release checks"
	@echo "  help           - Show this help message"
