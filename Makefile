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
	LOCK_VERSION=$$(awk '/^name = "edgefirst-replay"$$/ {found=1} found && /^version = / {gsub(/version = "|"/, ""); print; exit}' Cargo.lock); \
	echo "Cargo.toml version: $$VERSION"; \
	echo "Cargo.lock version: $$LOCK_VERSION"; \
	if [ -z "$$VERSION" ] || [ -z "$$LOCK_VERSION" ]; then \
		echo "ERROR: Could not read version from Cargo.toml or Cargo.lock"; \
		exit 1; \
	fi; \
	if [ "$$VERSION" != "$$LOCK_VERSION" ]; then \
		echo "ERROR: Version mismatch between Cargo.toml ($$VERSION) and Cargo.lock ($$LOCK_VERSION)"; \
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
