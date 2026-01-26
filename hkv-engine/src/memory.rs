//! # In-Memory Engine
//!
//! Provide the in-memory backend with sharded locking, TTL-aware
//! lookups, and byte-based LRU eviction for predictable latency.
//!
//! ## Usage
//!
//! - Use `MemoryEngine::new()` for a default sharded engine with unlimited
//!   capacity (Phase 1 baseline).
//! - Use `MemoryEngine::with_shard_count_and_capacity` to enforce a byte limit
//!   and trigger LRU eviction.
//! - Use `start_expirer` to enable active TTL cleanup in the background.
//!
//! ## Design Principles
//!
//! 1. **Sharded Locks**: Per-shard locks reduce contention under concurrency.
//! 2. **Byte-Based LRU**: Evict by total bytes to enforce memory limits.
//! 3. **Arc-backed Buffers**: Values are `Arc<[u8]>` to avoid extra copies.
//! 4. **TTL Fast Path**: Expiration is checked on access for O(1) reads.
//! 5. **Strategy Pattern**: Implements `KVEngine` to keep callers decoupled.
//!
//! ## Structure Overview
//!
//! The engine wires shards, locks, and LRU nodes together as follows:
//!
//! ```text
//! MemoryEngine
//!   └── shards: Vec<Shard>
//!         └── Shard
//!               └── inner: RwLock<ShardInner>
//!                     ├── map: HashMap<Arc<[u8]>, usize>
//!                     ├── nodes: Vec<Option<Node>>
//!                     ├── free: Vec<usize>
//!                     └── head/tail: LRU indices
//!                           └── Node { key, value, expires_at, size, prev, next }
//! ```

use std::hash::{BuildHasher, Hasher};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ahash::RandomState;
use hashbrown::HashMap;
use parking_lot::RwLock;

use hkv_common::{HkvError, HkvResult};

use crate::engine::{KVEngine, TtlStatus};

/// Default shards = CPU count * multiplier to reduce lock contention.
const DEFAULT_SHARD_MULTIPLIER: usize = 4;

/// Internal node representing a single key/value entry.
///
/// Uses an index-based intrusive list (pattern) for O(1) LRU updates without
/// heap pointers, keeping the layout cache-friendly and safe.
#[derive(Debug)]
struct Node {
    // Shared key buffer; map stores the same Arc to avoid duplicate allocations.
    key: Arc<[u8]>,
    // Shared value buffer for zero-copy reads across callers.
    value: Arc<[u8]>,
    // Absolute expiration timestamp.
    expires_at: Option<Instant>,
    // Byte size for eviction accounting (key + value).
    size: usize,
    // Intrusive LRU pointers (index-based to keep nodes packed).
    prev: Option<usize>,
    next: Option<usize>,
}

impl Node {
    /// Returns true when the entry has expired at `now`.
    ///
    /// Used on access to keep the hot path simple and predictable.
    fn is_expired(&self, now: Instant) -> bool {
        match self.expires_at {
            Some(deadline) => now >= deadline,
            None => false,
        }
    }
}

/// Per-shard storage container for the in-memory engine.
///
/// This struct keeps the hot path tightly packed: a hash map for lookups and a
/// dense node arena for LRU ordering. The arena stores indices for LRU links,
/// avoiding pointers and keeping data cache-friendly.
///
/// Design notes:
/// - The map key is `Arc<[u8]>` to share the key buffer with the node without
///   copying; this is a zero-cost abstraction because `Arc` is ref-counted.
/// - LRU links use indices instead of pointers to avoid unsafe code and keep
///   the layout stable for the compiler.
/// - `free` is a simple slot recycler to reduce allocations on churn.
#[derive(Debug)]
struct ShardInner {
    /// Key -> node index for O(1) lookup.
    map: HashMap<Arc<[u8]>, usize, RandomState>,
    /// Dense node storage for cache-friendly scans.
    nodes: Vec<Option<Node>>,
    /// Free-list for recycling node slots.
    free: Vec<usize>,
    /// LRU head (oldest) and tail (most recent).
    head: Option<usize>,
    tail: Option<usize>,
}

impl ShardInner {
    /// Creates a new shard with empty LRU state and a local hash map.
    ///
    /// Sharing the `RandomState` seed across shards keeps hash distribution
    /// consistent without introducing shared mutability.
    fn new(hash_state: RandomState) -> Self {
        ShardInner {
            map: HashMap::with_hasher(hash_state),
            nodes: Vec::new(),
            free: Vec::new(),
            head: None,
            tail: None,
        }
    }

    /// Detaches `idx` from the LRU list.
    ///
    /// Call this before re-linking or removing the node.
    fn lru_remove(&mut self, idx: usize) {
        let (prev, next) = {
            let node = self.nodes[idx].as_ref().expect("node exists");
            (node.prev, node.next)
        };

        if let Some(prev_idx) = prev {
            if let Some(prev_node) = self.nodes[prev_idx].as_mut() {
                prev_node.next = next;
            }
        } else {
            self.head = next;
        }

        if let Some(next_idx) = next {
            if let Some(next_node) = self.nodes[next_idx].as_mut() {
                next_node.prev = prev;
            }
        } else {
            self.tail = prev;
        }

        if let Some(node) = self.nodes[idx].as_mut() {
            node.prev = None;
            node.next = None;
        }
    }

    /// Appends `idx` to the LRU tail (most recently used).
    ///
    /// This keeps updates O(1) without heap pointers.
    fn lru_push_back(&mut self, idx: usize) {
        let tail = self.tail;
        if let Some(node) = self.nodes[idx].as_mut() {
            node.prev = tail;
            node.next = None;
        }

        if let Some(tail_idx) = tail {
            if let Some(tail_node) = self.nodes[tail_idx].as_mut() {
                tail_node.next = Some(idx);
            }
        } else {
            self.head = Some(idx);
        }

        self.tail = Some(idx);
    }

    /// Marks a node as recently used by moving it to the tail.
    ///
    /// Skips relinking if the node is already the tail.
    fn touch(&mut self, idx: usize) {
        if self.tail == Some(idx) {
            return;
        }
        self.lru_remove(idx);
        self.lru_push_back(idx);
    }

    /// Inserts a new node and returns its slot index.
    ///
    /// Reuses a free slot if available to reduce allocations under churn.
    fn insert_new(&mut self, key: Arc<[u8]>, value: Arc<[u8]>, size: usize) -> usize {
        let idx = self.free.pop().unwrap_or_else(|| {
            self.nodes.push(None);
            self.nodes.len() - 1
        });

        self.nodes[idx] = Some(Node {
            key: Arc::clone(&key),
            value,
            expires_at: None,
            size,
            prev: None,
            next: None,
        });
        self.lru_push_back(idx);
        self.map.insert(key, idx);
        idx
    }

    /// Removes a node by index and returns its byte size.
    ///
    /// This updates the map, LRU links, and free list.
    fn remove_idx(&mut self, idx: usize) -> Option<usize> {
        let node = self.nodes[idx].as_ref()?;
        let key = Arc::clone(&node.key);
        let size = node.size;

        // Detach before clearing the slot so LRU pointers stay valid.
        self.lru_remove(idx);
        self.nodes[idx] = None;
        self.map.remove(key.as_ref());
        self.free.push(idx);
        Some(size)
    }

    /// Removes and returns the least-recently used node size.
    ///
    /// Used by the eviction logic when over capacity.
    fn pop_lru(&mut self) -> Option<usize> {
        let idx = self.head?;
        self.remove_idx(idx)
    }
}

/// Per-shard lock wrapper.
///
/// Encapsulates shard state so locking stays localized to one shard.
#[derive(Debug)]
struct Shard {
    /// Per-shard lock to reduce contention on multi-core workloads.
    inner: RwLock<ShardInner>,
}

/// Sharded in-memory implementation of `KVEngine` for Phase 1.
///
/// This engine favors predictable latency and cache locality over feature
/// richness; it only supports string keys/values for now.
#[derive(Debug)]
pub struct MemoryEngine {
    /// Per-shard storage.
    shards: Vec<Shard>,
    /// Bitmask for fast shard selection (power-of-two shard count).
    shard_mask: usize,
    /// Hash state used to pick shards deterministically.
    hash_state: RandomState,
    /// Maximum allowed bytes before eviction starts.
    max_bytes: usize,
    /// Global byte usage, updated on insert/remove.
    used_bytes: AtomicUsize,
    /// Round-robin cursor for eviction across shards.
    eviction_cursor: AtomicUsize,
}

/// Handle for the background expiration sweeper.
///
/// Call `stop` to signal shutdown and join the thread.
pub struct ExpirationHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ExpirationHandle {
    /// Stops the sweeper and waits for the thread to finish.
    ///
    /// Use this in tests or shutdown hooks to avoid leaking threads.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl MemoryEngine {
    /// Creates a new engine with a default shard count based on CPU parallelism.
    ///
    /// Uses an effectively unbounded capacity to keep Phase 1 simple.
    pub fn new() -> Self {
        let threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);
        let shard_count = threads.saturating_mul(DEFAULT_SHARD_MULTIPLIER);
        Self::with_shard_count(shard_count)
    }

    /// Creates a new engine with a caller-provided shard count.
    ///
    /// The count is normalized to the next power of two to enable fast masking.
    pub fn with_shard_count(shards: usize) -> Self {
        Self::with_shard_count_and_capacity(shards, usize::MAX)
    }

    /// Creates a new engine with shard count and a byte capacity limit.
    ///
    /// Eviction triggers when `used_bytes` exceeds `max_bytes`.
    pub fn with_shard_count_and_capacity(shards: usize, max_bytes: usize) -> Self {
        let shard_count = normalize_shard_count(shards);
        let hash_state = RandomState::new();
        let mut shard_vec = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shard_vec.push(Shard {
                inner: RwLock::new(ShardInner::new(hash_state.clone())),
            });
        }

        MemoryEngine {
            shards: shard_vec,
            shard_mask: shard_count - 1,
            hash_state,
            max_bytes,
            used_bytes: AtomicUsize::new(0),
            eviction_cursor: AtomicUsize::new(0),
        }
    }

    /// Removes expired entries across all shards.
    ///
    /// This is an O(n) scan and is intended for a periodic background sweep.
    pub fn purge_expired(&self, now: Instant) -> usize {
        let mut removed = 0;
        for shard in &self.shards {
            let mut inner = shard.inner.write();
            let mut expired = Vec::new();
            for &idx in inner.map.values() {
                if let Some(node) = inner.nodes[idx].as_ref() {
                    if node.is_expired(now) {
                        expired.push(idx);
                    }
                }
            }

            for idx in expired {
                if let Some(size) = inner.remove_idx(idx) {
                    removed += 1;
                    self.used_bytes.fetch_sub(size, Ordering::Relaxed);
                }
            }
        }
        removed
    }

    /// Starts a background thread that periodically removes expired entries.
    ///
    /// The returned handle must be stopped to avoid leaking the thread.
    pub fn start_expirer(self: &Arc<Self>, interval: Duration) -> ExpirationHandle {
        let interval = if interval.is_zero() {
            Duration::from_millis(1)
        } else {
            interval
        };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let engine = Arc::clone(self);

        let join = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Acquire) {
                std::thread::sleep(interval);
                engine.purge_expired(Instant::now());
            }
        });

        ExpirationHandle {
            stop,
            join: Some(join),
        }
    }

    /// Hashes a key to its owning shard index.
    ///
    /// Uses the same hash state as the shard map to keep distribution uniform.
    fn shard_index(&self, key: &[u8]) -> usize {
        let mut hasher = self.hash_state.build_hasher();
        hasher.write(key);
        (hasher.finish() as usize) & self.shard_mask
    }

    /// Returns the shard responsible for a given key.
    fn shard_for(&self, key: &[u8]) -> &Shard {
        &self.shards[self.shard_index(key)]
    }

    /// Calculates entry size for eviction accounting.
    ///
    /// This ignores allocator overhead to keep the computation zero-cost.
    fn entry_size(key_len: usize, value_len: usize) -> usize {
        key_len + value_len
    }

    /// Evicts entries until within the configured byte budget.
    ///
    /// Scans shards in round-robin order to avoid concentrating evictions.
    fn evict_if_needed(&self) {
        if self.max_bytes == usize::MAX {
            return;
        }

        loop {
            let used = self.used_bytes.load(Ordering::Relaxed);
            if used <= self.max_bytes {
                break;
            }

            let start = self.eviction_cursor.fetch_add(1, Ordering::Relaxed);
            let mut evicted = false;

            for offset in 0..self.shards.len() {
                let idx = (start + offset) & self.shard_mask;
                if let Some(size) = self.evict_one_from_shard(idx) {
                    self.used_bytes.fetch_sub(size, Ordering::Relaxed);
                    evicted = true;
                    break;
                }
            }

            if !evicted {
                break;
            }
        }
    }

    /// Evicts a single LRU entry from a shard.
    ///
    /// Returns the reclaimed byte size for global accounting.
    fn evict_one_from_shard(&self, shard_index: usize) -> Option<usize> {
        let shard = &self.shards[shard_index];
        let mut inner = shard.inner.write();
        inner.pop_lru()
    }
}

impl KVEngine for MemoryEngine {
    /// Looks up a key, updates LRU, and returns its value if present.
    ///
    /// Expired entries are removed on access to keep memory usage stable.
    fn get(&self, key: &[u8]) -> HkvResult<Option<Arc<[u8]>>> {
        let shard = self.shard_for(key);
        let now = Instant::now();
        let mut inner = shard.inner.write();

        let idx = match inner.map.get(key) {
            Some(&idx) => idx,
            None => return Ok(None),
        };

        let expired = match inner.nodes[idx].as_ref() {
            Some(node) => node.is_expired(now),
            None => return Ok(None),
        };

        if expired {
            if let Some(size) = inner.remove_idx(idx) {
                self.used_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            return Ok(None);
        }

        let value = inner.nodes[idx]
            .as_ref()
            .map(|node| Arc::clone(&node.value));
        inner.touch(idx);
        Ok(value)
    }

    /// Inserts or replaces a key/value pair and updates LRU ordering.
    ///
    /// This resets TTL to `None` and triggers eviction when over budget.
    fn set(&self, key: Vec<u8>, value: Vec<u8>) -> HkvResult<()> {
        let shard = self.shard_for(&key);
        let mut inner = shard.inner.write();
        let key_arc: Arc<[u8]> = Arc::from(key);
        let value_arc: Arc<[u8]> = Arc::from(value);
        let new_size = Self::entry_size(key_arc.len(), value_arc.len());

        if let Some(&idx) = inner.map.get(key_arc.as_ref()) {
            let remove = inner.nodes[idx].as_ref().map(|node| node.is_expired(Instant::now()));
            if remove.unwrap_or(false) {
                if let Some(size) = inner.remove_idx(idx) {
                    self.used_bytes.fetch_sub(size, Ordering::Relaxed);
                }
            }
        }

        if let Some(&idx) = inner.map.get(key_arc.as_ref()) {
            if let Some(node) = inner.nodes[idx].as_mut() {
                let old_size = node.size;
                node.value = value_arc;
                node.size = new_size;
                node.expires_at = None;
                inner.touch(idx);

                if new_size > old_size {
                    self.used_bytes
                        .fetch_add(new_size - old_size, Ordering::Relaxed);
                } else if old_size > new_size {
                    self.used_bytes
                        .fetch_sub(old_size - new_size, Ordering::Relaxed);
                }
            }
        } else {
            inner.insert_new(Arc::clone(&key_arc), value_arc, new_size);
            self.used_bytes.fetch_add(new_size, Ordering::Relaxed);
        }

        drop(inner);
        self.evict_if_needed();
        Ok(())
    }

    /// Deletes a key and returns whether a live entry was removed.
    ///
    /// Expired entries are treated as missing to match Redis semantics.
    fn delete(&self, key: &[u8]) -> HkvResult<bool> {
        let shard = self.shard_for(key);
        let now = Instant::now();
        let mut inner = shard.inner.write();

        let idx = match inner.map.get(key) {
            Some(&idx) => idx,
            None => return Ok(false),
        };

        let expired = inner.nodes[idx]
            .as_ref()
            .map(|node| node.is_expired(now))
            .unwrap_or(false);

        if let Some(size) = inner.remove_idx(idx) {
            self.used_bytes.fetch_sub(size, Ordering::Relaxed);
        }

        Ok(!expired)
    }

    /// Sets a TTL for an existing key.
    ///
    /// Missing or expired keys return `HkvError::NotFound`.
    fn expire(&self, key: &[u8], ttl: Duration) -> HkvResult<()> {
        let shard = self.shard_for(key);
        let now = Instant::now();
        let mut inner = shard.inner.write();

        let idx = match inner.map.get(key) {
            Some(&idx) => idx,
            None => return Err(HkvError::NotFound),
        };

        let expired = inner.nodes[idx]
            .as_ref()
            .map(|node| node.is_expired(now))
            .unwrap_or(false);

        if expired {
            if let Some(size) = inner.remove_idx(idx) {
                self.used_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            return Err(HkvError::NotFound);
        }

        if let Some(node) = inner.nodes[idx].as_mut() {
            node.expires_at = Some(now + ttl);
        }

        Ok(())
    }

    /// Returns TTL state for a key (missing, no-expiry, or remaining time).
    ///
    /// This mirrors Redis `TTL` semantics for the server layer.
    fn ttl(&self, key: &[u8]) -> HkvResult<TtlStatus> {
        let shard = self.shard_for(key);
        let now = Instant::now();
        let mut inner = shard.inner.write();

        let idx = match inner.map.get(key) {
            Some(&idx) => idx,
            None => return Ok(TtlStatus::Missing),
        };

        let expired = inner.nodes[idx]
            .as_ref()
            .map(|node| node.is_expired(now))
            .unwrap_or(false);

        if expired {
            if let Some(size) = inner.remove_idx(idx) {
                self.used_bytes.fetch_sub(size, Ordering::Relaxed);
            }
            return Ok(TtlStatus::Missing);
        }

        let expires_at = inner.nodes[idx].as_ref().and_then(|node| node.expires_at);
        match expires_at {
            None => Ok(TtlStatus::NoExpiry),
            Some(deadline) => {
                if deadline <= now {
                    if let Some(size) = inner.remove_idx(idx) {
                        self.used_bytes.fetch_sub(size, Ordering::Relaxed);
                    }
                    return Ok(TtlStatus::Missing);
                }
                Ok(TtlStatus::ExpiresIn(deadline - now))
            }
        }
    }
}

/// Normalizes shard counts to a power of two for fast masking.
///
/// This keeps shard selection branch-free and avoids modulo operations.
fn normalize_shard_count(count: usize) -> usize {
    let count = count.max(1);
    count.next_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip() {
        let engine = MemoryEngine::with_shard_count(4);
        engine.set(b"alpha".to_vec(), b"value".to_vec()).unwrap();
        let value = engine.get(b"alpha").unwrap().unwrap();
        assert_eq!(&*value, b"value");
    }

    #[test]
    fn delete_removes_key() {
        let engine = MemoryEngine::with_shard_count(2);
        engine.set(b"alpha".to_vec(), b"value".to_vec()).unwrap();
        assert!(engine.delete(b"alpha").unwrap());
        assert!(engine.get(b"alpha").unwrap().is_none());
    }

    #[test]
    fn expire_hides_value() {
        let engine = MemoryEngine::with_shard_count(2);
        engine.set(b"alpha".to_vec(), b"value".to_vec()).unwrap();
        engine.expire(b"alpha", Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(engine.get(b"alpha").unwrap().is_none());
    }

    #[test]
    fn purge_expired_removes_entries() {
        let engine = MemoryEngine::with_shard_count(2);
        engine.set(b"alpha".to_vec(), b"value".to_vec()).unwrap();
        engine.expire(b"alpha", Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(5));

        let removed = engine.purge_expired(Instant::now());
        assert_eq!(removed, 1);
        assert!(engine.get(b"alpha").unwrap().is_none());
    }

    #[test]
    fn expirer_thread_clears_expired() {
        let engine = Arc::new(MemoryEngine::with_shard_count(2));
        engine.set(b"alpha".to_vec(), b"value".to_vec()).unwrap();
        engine.expire(b"alpha", Duration::from_millis(1)).unwrap();

        let handle = engine.start_expirer(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        handle.stop();

        assert!(engine.get(b"alpha").unwrap().is_none());
    }

    #[test]
    fn evicts_lru_by_bytes() {
        let engine = MemoryEngine::with_shard_count_and_capacity(1, 10);
        engine.set(b"a".to_vec(), b"1234".to_vec()).unwrap();
        engine.set(b"b".to_vec(), b"1234".to_vec()).unwrap();
        engine.get(b"a").unwrap();
        engine.set(b"c".to_vec(), b"1234".to_vec()).unwrap();

        assert!(engine.get(b"b").unwrap().is_none());
        assert!(engine.get(b"a").unwrap().is_some());
        assert!(engine.get(b"c").unwrap().is_some());
    }

    #[test]
    fn ttl_reports_missing_or_expiry() {
        let engine = MemoryEngine::with_shard_count(2);
        assert_eq!(engine.ttl(b"missing").unwrap(), TtlStatus::Missing);

        engine.set(b"alpha".to_vec(), b"value".to_vec()).unwrap();
        assert_eq!(engine.ttl(b"alpha").unwrap(), TtlStatus::NoExpiry);

        engine.expire(b"alpha", Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(engine.ttl(b"alpha").unwrap(), TtlStatus::Missing);
    }
}
