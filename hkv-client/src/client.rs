//! # Synchronous Client API
//!
//! Purpose: Expose a compact, blocking API for issuing Redis-compatible
//! commands to the HybridKV server over RESP2.
//!
//! ## Design Principles
//! 1. **Facade Pattern**: `KVClient` hides pooling and protocol details.
//! 2. **Borrow-Friendly API**: Accept `&[u8]` to avoid unnecessary copies.
//! 3. **Fail Fast**: Protocol violations surface immediately as errors.
//! 4. **Performance First**: Prefer direct TCP writes and buffer reuse.

use std::fmt;
use std::time::Duration;

use crate::pool::{ConnectionPool, PoolConfig};
use crate::resp::RespValue;

/// Result type for the sync client.
pub type ClientResult<T> = Result<T, ClientError>;

/// Errors surfaced by the sync client.
#[derive(Debug)]
pub enum ClientError {
    /// Network or IO failure while reading/writing.
    Io(std::io::Error),
    /// RESP2 framing or parse error.
    Protocol,
    /// Server returned an error reply.
    Server { message: Vec<u8> },
    /// Response type did not match the expected command response.
    UnexpectedResponse,
    /// Pool is at capacity and no idle connections are available.
    PoolExhausted,
    /// Address could not be parsed into a socket address.
    InvalidAddress,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Io(err) => write!(f, "io error: {}", err),
            ClientError::Protocol => write!(f, "protocol error"),
            ClientError::Server { message } => {
                write!(f, "server error: {}", String::from_utf8_lossy(message))
            }
            ClientError::UnexpectedResponse => write!(f, "unexpected response"),
            ClientError::PoolExhausted => write!(f, "connection pool exhausted"),
            ClientError::InvalidAddress => write!(f, "invalid address"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        ClientError::Io(err)
    }
}

/// TTL state returned by the server, mirroring Redis semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientTtl {
    /// Key is missing or already expired.
    Missing,
    /// Key exists without expiration.
    NoExpiry,
    /// Key expires after the provided duration.
    ExpiresIn(Duration),
}

/// Configuration for the synchronous client and its pool.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Server address, e.g. "127.0.0.1:6379".
    pub addr: String,
    /// Maximum idle connections kept in the pool.
    pub max_idle: usize,
    /// Maximum total connections (idle + in-use).
    pub max_total: usize,
    /// Optional TCP read timeout.
    pub read_timeout: Option<Duration>,
    /// Optional TCP write timeout.
    pub write_timeout: Option<Duration>,
    /// Optional TCP connect timeout.
    pub connect_timeout: Option<Duration>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            addr: "127.0.0.1:6379".to_string(),
            max_idle: 8,
            max_total: 16,
            read_timeout: None,
            write_timeout: None,
            connect_timeout: None,
        }
    }
}

/// Synchronous client with connection pooling.
///
/// This is a facade over the pool and RESP encoder/decoder. Each call acquires
/// a connection, executes one command, and returns the connection to the pool.
pub struct KVClient {
    pool: ConnectionPool,
}

impl KVClient {
    /// Creates a client with default configuration.
    pub fn connect(addr: impl Into<String>) -> ClientResult<Self> {
        let mut config = ClientConfig::default();
        config.addr = addr.into();
        Self::with_config(config)
    }

    /// Creates a client with a custom configuration.
    pub fn with_config(config: ClientConfig) -> ClientResult<Self> {
        let pool = ConnectionPool::new(PoolConfig {
            addr: config.addr,
            max_idle: config.max_idle,
            max_total: config.max_total,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            connect_timeout: config.connect_timeout,
        })?;
        Ok(KVClient { pool })
    }

    /// Fetches a value by key.
    ///
    /// Returns `Ok(None)` when the key is missing.
    pub fn get(&self, key: &[u8]) -> ClientResult<Option<Vec<u8>>> {
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"GET", key])? {
            RespValue::Bulk(data) => Ok(data),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Sets a value for a key without expiration.
    pub fn set(&self, key: &[u8], value: &[u8]) -> ClientResult<()> {
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"SET", key, value])? {
            RespValue::Simple(_) => Ok(()),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Sets a value and attaches an expiration in seconds.
    pub fn set_with_ttl(&self, key: &[u8], value: &[u8], ttl: Duration) -> ClientResult<()> {
        let (seconds, len) = encode_u64(ttl.as_secs());
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"SET", key, value, b"EX", &seconds[..len]])? {
            RespValue::Simple(_) => Ok(()),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Deletes a key. Returns true when a key was removed.
    pub fn delete(&self, key: &[u8]) -> ClientResult<bool> {
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"DEL", key])? {
            RespValue::Integer(count) => Ok(count > 0),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Sets a time-to-live on a key. Returns true when the TTL was set.
    pub fn expire(&self, key: &[u8], ttl: Duration) -> ClientResult<bool> {
        let (seconds, len) = encode_u64(ttl.as_secs());
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"EXPIRE", key, &seconds[..len]])? {
            RespValue::Integer(value) => Ok(value == 1),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Returns TTL status for a key.
    pub fn ttl(&self, key: &[u8]) -> ClientResult<ClientTtl> {
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"TTL", key])? {
            RespValue::Integer(value) if value == -2 => Ok(ClientTtl::Missing),
            RespValue::Integer(value) if value == -1 => Ok(ClientTtl::NoExpiry),
            RespValue::Integer(value) if value >= 0 => {
                Ok(ClientTtl::ExpiresIn(Duration::from_secs(value as u64)))
            }
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Pings the server. Returns the raw response payload.
    pub fn ping(&self, payload: Option<&[u8]>) -> ClientResult<Vec<u8>> {
        let mut conn = self.pool.acquire()?;
        let response = match payload {
            Some(data) => conn.exec(&[b"PING", data])?,
            None => conn.exec(&[b"PING"])?,
        };
        match response {
            RespValue::Simple(text) => Ok(text),
            RespValue::Bulk(Some(data)) => Ok(data),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Fetches server INFO output.
    pub fn info(&self) -> ClientResult<Vec<u8>> {
        let mut conn = self.pool.acquire()?;
        match conn.exec(&[b"INFO"])? {
            RespValue::Bulk(Some(data)) => Ok(data),
            RespValue::Error(message) => Err(ClientError::Server { message }),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }
}

fn encode_u64(mut value: u64) -> ([u8; 20], usize) {
    // Stack buffer keeps conversion allocation-free (zero-cost abstraction).
    let mut buf = [0u8; 20];
    let mut len = 0;
    if value == 0 {
        buf[0] = b'0';
        return (buf, 1);
    }
    while value > 0 {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    buf[..len].reverse();
    (buf, len)
}
