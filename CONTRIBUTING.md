# Contributing to EdgeFirst Replay

Thank you for your interest in contributing to EdgeFirst Replay! This document
provides guidelines for contributing to this project.

## Code of Conduct

This project adheres to the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).
By participating, you are expected to uphold this code. Please report
unacceptable behavior to support@au-zone.com.

## Getting Started

Before contributing:

1. Read the [README.md](README.md) to understand the project
2. Review the [ARCHITECTURE.md](ARCHITECTURE.md) for system design
3. Check existing [issues](https://github.com/EdgeFirstAI/replay/issues)
   and [discussions](https://github.com/EdgeFirstAI/replay/discussions)

### Ways to Contribute

- **Code**: Bug fixes, new features, performance improvements
- **Documentation**: README improvements, code comments, examples
- **Testing**: Unit tests, integration tests, validation scripts
- **Community**: Answer questions, write tutorials, share use cases

## Development Setup

### Prerequisites

- **Rust**: 1.70 or later
- **Git**: For version control
- **Cross-compilation tools** (optional): `gcc-aarch64-linux-gnu` for ARM64 builds

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/EdgeFirstAI/replay.git
cd replay

# Build
cargo build

# Run tests
cargo test

# Format code
cargo fmt

# Run linter
cargo clippy
```

## How to Contribute

### Reporting Bugs

Before creating bug reports, please check existing issues to avoid duplicates.

**Good Bug Reports** include:

- Clear, descriptive title
- Steps to reproduce the behavior
- Expected vs. actual behavior
- Environment details (OS, Rust version, target platform)
- Minimal code example demonstrating the issue
- Screenshots if applicable

### Suggesting Enhancements

Enhancement suggestions are tracked as GitHub issues. Provide:

- Clear, descriptive title
- Detailed description of the proposed functionality
- Use cases and motivation
- Examples of how the feature would be used
- Possible implementation approach (optional)

### Contributing Code

1. **Fork the repository** and create your branch from `main`
2. **Make your changes** following our code style guidelines
3. **Add tests** for new functionality (minimum 70% coverage)
4. **Ensure all tests pass** (`cargo test`)
5. **Update documentation** for API changes
6. **Run formatters and linters** (`cargo fmt`, `cargo clippy`)
7. **Submit a pull request** with a clear description

## Code Style Guidelines

- Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `cargo fmt` (enforced in CI)
- Address all `cargo clippy` warnings
- Write doc comments for public APIs
- Maximum line length: 100 characters
- Use descriptive variable names

## Dependency Management

### Adding New Dependencies

When adding dependencies to `Cargo.toml`:

1. **Check License Compatibility**
   - Only use permissive licenses: MIT, Apache-2.0, BSD (2/3-clause), ISC, EPL-2.0
   - Avoid GPL, AGPL, or restrictive licenses in Rust (static linking)

2. **Update Cargo.lock**

   ```bash
   cargo build
   git add Cargo.lock
   ```

3. **Update NOTICE File** if adding new first-level dependencies

## Testing Requirements

### Testing Guidelines

- **Unit Tests**: Add tests for new functionality where practical
- **Hardware Dependencies**: Many components require NXP i.MX8M Plus hardware (VPU, G2D, DMA)
- Critical paths and argument parsing should have test coverage

### Running Tests

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_name
```

## Pull Request Process

### Branch Naming

```text
feature/<description>       # New features
bugfix/<description>        # Bug fixes
docs/<description>          # Documentation updates
```

### Commit Messages

Write clear, concise commit messages:

```text
Add replay speed validation

- Validate replay_speed is positive
- Add error message for invalid values
- Add unit test for validation

Signed-off-by: Your Name <your.email@example.com>
```

### Pull Request Checklist

Before submitting, ensure:

- [ ] Code follows style guidelines (`cargo fmt`, `cargo clippy`)
- [ ] All tests pass (`cargo test`)
- [ ] New tests added for new functionality
- [ ] Documentation updated for API changes
- [ ] Commit messages are clear and descriptive
- [ ] Branch is up-to-date with `main`
- [ ] PR description clearly explains changes
- [ ] SPDX headers present in new files
- [ ] All commits have DCO sign-off

## Developer Certificate of Origin (DCO)

All contributors must sign off their commits to certify they have the right to
submit the code under the project's open source license.

### How to Sign Off Commits

```bash
git commit -s -m "Add new feature"
```

This adds: `Signed-off-by: Your Name <your.email@example.com>`

**Configure git:**

```bash
git config user.name "Your Name"
git config user.email "your.email@example.com"
```

## License

By contributing to EdgeFirst Replay, you agree that your contributions will be
licensed under the [Apache License 2.0](LICENSE).

All source files must include the SPDX license header:

```rust
// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0
```

## Questions?

- **Discussions**: https://github.com/EdgeFirstAI/replay/discussions
- **Issues**: https://github.com/EdgeFirstAI/replay/issues
- **Email**: support@au-zone.com

Thank you for helping make EdgeFirst Replay better!
