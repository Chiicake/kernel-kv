//! # HybridKV Sync Client
//!
//! Provide a lightweight, synchronous Redis-compatible client with
//! connection pooling to minimize TCP handshake overhead.

mod client;
mod pool;
mod resp;

pub use client::{ClientConfig, ClientError, ClientResult, ClientTtl, KVClient};
