//! # HybridKV Server
//!
//! Provide a Redis-compatible TCP server that routes commands to the
//! user-space storage engine.
//!
//! ## Design Principles
//!
//! 1. **Single Responsibility**: Parsing and dispatch are isolated in modules.
//! 2. **Async First**: Tokio handles concurrent connections efficiently.
//! 3. **Fail-Open Defaults**: Protocol errors are localized to the connection.
//! 4. **Performance Focus**: Reuse buffers and avoid unnecessary allocations.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use hkv_engine::MemoryEngine;
use hkv_server::metrics::Metrics;
use hkv_server::server;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("HKV_ADDR").unwrap_or_else(|_| "127.0.0.1:6379".to_string());
    let listener = TcpListener::bind(&addr).await?;

    let engine = Arc::new(MemoryEngine::new());
    let metrics = Arc::new(Metrics::new());
    let _expirer = engine.start_expirer(Duration::from_secs(1));

    loop {
        let (stream, _) = listener.accept().await?;
        let engine = Arc::clone(&engine);
        let metrics = Arc::clone(&metrics);
        tokio::spawn(async move {
            let _ = server::handle_connection_with_metrics(stream, engine, metrics).await;
        });
    }
}
