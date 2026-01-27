//! # Connection Pool
//!
//! Purpose: Reuse TCP connections for the sync client to reduce handshake
//! latency and allocation churn.
//!
//! ## Design Principles
//! 1. **Object Pool Pattern**: Keep a bounded set of reusable connections.
//! 2. **Minimal Locking**: Hold the mutex only while moving idle connections.
//! 3. **Fail Fast**: Exceeding the pool limit returns an error immediately.
//! 4. **Cache-Friendly Buffers**: Each connection reuses its own buffers.

use std::collections::VecDeque;
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::client::{ClientError, ClientResult};
use crate::resp::{encode_command, read_response, RespValue};

/// Pool configuration for the sync client.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Server address, e.g. "127.0.0.1:6379".
    pub addr: String,
    /// Maximum number of idle connections to keep.
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

struct PoolState {
    idle: VecDeque<Connection>,
    total: usize,
}

struct PoolInner {
    config: PoolConfig,
    state: Mutex<PoolState>,
}

/// Connection pool handle.
#[derive(Clone)]
pub struct ConnectionPool {
    inner: Arc<PoolInner>,
}

impl ConnectionPool {
    /// Creates a new connection pool with the provided configuration.
    pub fn new(config: PoolConfig) -> ClientResult<Self> {
        let state = PoolState {
            idle: VecDeque::with_capacity(config.max_idle),
            total: 0,
        };
        Ok(ConnectionPool {
            inner: Arc::new(PoolInner {
                config,
                state: Mutex::new(state),
            }),
        })
    }

    /// Acquires a connection from the pool.
    pub fn acquire(&self) -> ClientResult<PooledConnection> {
        if let Some(conn) = self.pop_idle() {
            return Ok(PooledConnection::new(self.inner.clone(), conn));
        }

        if !self.try_reserve() {
            return Err(ClientError::PoolExhausted);
        }

        match Connection::connect(&self.inner.config) {
            Ok(conn) => Ok(PooledConnection::new(self.inner.clone(), conn)),
            Err(err) => {
                self.release_slot();
                Err(err)
            }
        }
    }

    fn pop_idle(&self) -> Option<Connection> {
        let mut state = self.inner.state.lock().expect("pool mutex poisoned");
        state.idle.pop_front()
    }

    fn try_reserve(&self) -> bool {
        let mut state = self.inner.state.lock().expect("pool mutex poisoned");
        if state.total >= self.inner.config.max_total {
            return false;
        }
        state.total += 1;
        true
    }

    fn release_slot(&self) {
        let mut state = self.inner.state.lock().expect("pool mutex poisoned");
        state.total = state.total.saturating_sub(1);
    }

    fn return_connection(&self, conn: Connection) {
        let mut state = self.inner.state.lock().expect("pool mutex poisoned");
        if state.idle.len() < self.inner.config.max_idle {
            state.idle.push_back(conn);
        } else {
            state.total = state.total.saturating_sub(1);
        }
    }
}

/// RAII wrapper returning a connection to the pool on drop.
pub struct PooledConnection {
    pool: Arc<PoolInner>,
    conn: Option<Connection>,
    valid: bool,
}

impl PooledConnection {
    fn new(pool: Arc<PoolInner>, conn: Connection) -> Self {
        PooledConnection {
            pool,
            conn: Some(conn),
            valid: true,
        }
    }

    /// Executes a RESP command and returns the parsed response.
    pub fn exec(&mut self, args: &[&[u8]]) -> ClientResult<RespValue> {
        let conn = self.conn.as_mut().expect("connection exists");
        let response = conn.exec(args);
        if response.is_err() {
            // If IO/protocol fails, do not return this connection to the pool.
            self.valid = false;
        }
        response
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        let conn = match self.conn.take() {
            Some(conn) => conn,
            None => return,
        };

        let pool = ConnectionPool {
            inner: self.pool.clone(),
        };

        if self.valid {
            pool.return_connection(conn);
        } else {
            pool.release_slot();
        }
    }
}

/// Single TCP connection with reusable buffers.
///
/// The buffers are stored on the connection to avoid per-call allocations.
pub struct Connection {
    // Buffered reader reduces syscalls while still allowing direct writes.
    reader: BufReader<TcpStream>,
    line_buf: Vec<u8>,
    write_buf: Vec<u8>,
}

impl Connection {
    fn connect(config: &PoolConfig) -> ClientResult<Self> {
        let stream = connect_stream(config)?;
        if let Some(timeout) = config.read_timeout {
            stream.set_read_timeout(Some(timeout))?;
        }
        if let Some(timeout) = config.write_timeout {
            stream.set_write_timeout(Some(timeout))?;
        }
        // Disable Nagle to keep request latency low for small payloads.
        stream.set_nodelay(true)?;

        Ok(Connection {
            reader: BufReader::new(stream),
            line_buf: Vec::with_capacity(128),
            write_buf: Vec::with_capacity(256),
        })
    }

    fn exec(&mut self, args: &[&[u8]]) -> ClientResult<RespValue> {
        self.write_buf.clear();
        encode_command(args, &mut self.write_buf);

        let stream = self.reader.get_mut();
        stream.write_all(&self.write_buf)?;
        stream.flush()?;

        read_response(&mut self.reader, &mut self.line_buf)
    }
}

fn connect_stream(config: &PoolConfig) -> ClientResult<TcpStream> {
    let addr: SocketAddr = config.addr.parse().map_err(|_| ClientError::InvalidAddress)?;
    let stream = match config.connect_timeout {
        Some(timeout) => TcpStream::connect_timeout(&addr, timeout)?,
        None => TcpStream::connect(addr)?,
    };
    Ok(stream)
}
