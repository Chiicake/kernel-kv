//! # HybridKV Sync Client
//!
//! Purpose: Provide a lightweight, synchronous Redis-compatible client with
//! connection pooling to minimize TCP handshake overhead.
//!
//! ## Design Principles
//! 1. **Object Pool Pattern**: Reuse TCP connections to avoid repeated connects.
//! 2. **Zero-Cost Abstractions**: Keep hot-path calls monomorphic and inline-friendly.
//! 3. **Minimal Allocation**: Reuse buffers for RESP framing and parsing.
//! 4. **Protocol Clarity**: Encode/parse RESP2 explicitly for correctness.

mod client;
mod pool;
mod resp;

pub use client::{ClientConfig, ClientError, ClientResult, ClientTtl, KVClient};
