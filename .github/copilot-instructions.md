# Copilot Instructions for HybridKV

This repository is a Rust workspace implementing a two-tier key-value system with a user-space engine and an intended kernel-resident cache.

## Build, Test, and Lint

### Build
- **Workspace Build**: `cargo build`
- **Specific Crate**: `cargo build -p <crate_name>` (e.g., `hkv-server`, `hkv-engine`)
- **Release Build**: `cargo build --release`

### Test
- **Run All Tests**: `cargo test`
- **Specific Crate**: `cargo test -p <crate_name>`
- **Single Test Case**: `cargo test -p <crate_name> -- <test_name_substring>`
  - Example: `cargo test -p hkv-server -- handle_connection`
- **Documentation Tests**: `cargo test --doc`

### Lint
- **Run Clippy**: `cargo clippy --workspace -- -D warnings`
- **Format Check**: `cargo fmt --check`

### Run
- **Start Server**: `cargo run -p hkv-server`
  - Default address: `127.0.0.1:6379`
  - Override address: `HKV_ADDR=0.0.0.0:6379 cargo run -p hkv-server`
- **Run Benchmarks**: `cargo run -p hkv-bench --release`

## High-Level Architecture

### Structure
The project is a Cargo workspace with the following core members:
- **hkv-engine**: The core in-memory storage engine logic (data structures, TTL, persistence).
- **hkv-server**: The network layer handling TCP connections, RESP protocol parsing, and command dispatch to `hkv-engine`. Built on `tokio`.
- **hkv-client**: A synchronous client library for interacting with the server.
- **hkv-common**: Shared types, error definitions, and protocols used across crates.
- **hkv-kernel**: (Experimental/Planned) Intended for kernel-resident caching strategies. Currently a placeholder/library.
- **hkv-bench**: Benchmarking tools.

### Key Components
1. **User Space (Authoritative)**:
   - `hkv-server` acts as the primary entry point.
   - Uses `tokio` for async I/O but delegates storage operations to `hkv-engine`.
   - `hkv-engine` manages the actual data in memory.

2. **Kernel Space (Cache)**:
   - The architectural vision includes a kernel module (`hkv-kernel`) to accelerate hot-key reads via `ioctl`/`netlink`.
   - *Note: This component is in early stages/scaffolding.*

## Key Conventions

- **Async vs Sync**:
  - `hkv-server` is **asynchronous** (Tokio-based).
  - `hkv-client` is **synchronous** (blocking I/O) to minimize overhead for simple use cases.
  - `hkv-engine` is generally synchronous/CPU-bound as it operates on in-memory data structures.

- **Configuration**:
  - Use environment variables for runtime configuration (e.g., `HKV_ADDR`).
  - Defaults should be robust (fail-open or safe defaults).

- **Protocol**:
  - The server implements the **RESP (Redis Serialization Protocol)**.
  - Commands and responses should adhere to Redis compatibility where possible.

- **Error Handling**:
  - Use `anyhow` or `thiserror` for error propagation.
  - Network errors should be handled gracefully without crashing the server.

- **Workspace Dependencies**:
  - Dependencies are managed at the workspace level in the root `Cargo.toml`.
  - Use `workspace = true` in member crates to inherit versions.
