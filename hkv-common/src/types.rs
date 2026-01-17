//! # Key/Value Type Definitions
//!
//! Core data types for HybridKV cache entries, designed for efficient user/kernel communication
//! and zero-copy operations where possible.
//!
//! ## Design Principles
//!
//! 1. **Fixed Maximum Sizes**: Keys (256B) and Values (1KB) have compile-time maximum sizes
//!    to enable stack allocation and predictable memory usage in kernel space (In kernel space, dynamic allocation (kmalloc) is slow, locks, fragmentation, oom risk, complex).
//!
//! 2. **C-Compatible Layout**: All types use `#[repr(C)]` for safe FFI with kernel module.
//!    This ensures predictable memory layout across user/kernel boundary.
//!
//! 3. **Variable-Length Data**: Uses length prefix + fixed buffer pattern to support
//!    variable-length data without heap allocation in kernel.
//!
//! 4. **Version Tracking**: Each entry has a monotonic version counter for consistency
//!    protocols (write-through invalidation, bounded staleness).
//!
//! 5. **TTL Support**: Time-to-live expiration with nanosecond precision using CLOCK_MONOTONIC.
//!
//! 6. **Len-Based Eq/Hash**: Compare and hash only initialized bytes to reduce cache traffic.
//!
//! ## Memory Layout Example
//!
//! ```text
//! Key (258 bytes total):
//! +--------+-----------+
//! | len:2B | data:256B |
//! +--------+-----------+
//!
//! Value (1026 bytes total):
//! +--------+------------+
//! | len:2B | data:1024B |
//! +--------+------------+
//!
//! EntryMetadata (40 bytes total, 8-byte aligned):
//! +--------+--------+-----------+------------+---------+--------------+
//! | ver:8B | ttl:8B | created:8B| accessed:8B| flags:1B| lens+pad:7B   |
//! +--------+--------+-----------+------------+---------+--------------+
//! Note: lens+pad = 1B padding + key_len(2B) + value_len(2B) + 2B padding.
//!
//! Entry (1328 bytes total):
//! +---------+------------+--------------+
//! | key:258B| value:1026B| metadata:40B |
//! +---------+------------+--------------+
//! Note: includes 4B padding between value and metadata.
//! ```

use std::fmt;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{HkvError, HkvResult};

/// Maximum key size in bytes (256 bytes)
pub const MAX_KEY_SIZE: usize = 256;

/// Maximum value size in bytes (1 KB)
pub const MAX_VALUE_SIZE: usize = 1024;

/// Key type with bounded size
///
/// Keys are limited to 256 bytes to:
/// - Enable stack allocation in kernel (no kmalloc in fast path)
/// - Fit in single cache line for hash computation
/// - Match typical Redis key sizes (most <100 bytes)
#[repr(C)]
#[derive(Clone)]
pub struct Key {
    /// Actual length of key data (≤ MAX_KEY_SIZE)
    len: u16,
    /// Key data buffer (only first `len` bytes are valid)
    data: [u8; MAX_KEY_SIZE],
}

// Compare only initialized bytes (length-prefixed buffer pattern).
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len.hash(state);
        self.as_bytes().hash(state);
    }
}

impl Key {
    /// Creates a new Key from byte slice
    ///
    /// # Errors
    /// Returns `HkvError::KeyTooLong` if data exceeds MAX_KEY_SIZE
    ///
    /// # Examples
    /// ```rust
    /// use hkv_common::{HkvError, MAX_KEY_SIZE};
    /// use hkv_common::types_copy::Key;
    ///
    /// let key = Key::new(b"alpha").expect("valid key");
    /// assert_eq!(key.as_bytes(), b"alpha");
    ///
    /// let too_long = vec![0u8; MAX_KEY_SIZE + 1];
    /// assert_eq!(Key::new(&too_long), Err(HkvError::KeyTooLong));
    /// ```
    pub fn new(data: &[u8]) -> HkvResult<Self> {
        if data.len() > MAX_KEY_SIZE {
            return Err(HkvError::KeyTooLong);
        }

        let mut key = Key {
            len: data.len() as u16,
            data: [0u8; MAX_KEY_SIZE],
        };
        key.data[..data.len()].copy_from_slice(data);
        Ok(key)
    }

    /// Returns the valid key data as a slice
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }

    /// Returns the key length
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns true if key is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:?})", String::from_utf8_lossy(self.as_bytes()))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(self.as_bytes()))
    }
}

/// Value type with bounded size
///
/// Values are limited to 1KB to:
/// - Keep hot entries in L3 cache (typical 2-20MB shared)
/// - Limit kernel memory footprint (256MB = ~250K entries)
/// - Encourage storing only hot small objects (large blobs stay in user-space)
#[repr(C)]
#[derive(Clone)]
pub struct Value {
    /// Actual length of value data (≤ MAX_VALUE_SIZE)
    len: u16,
    /// Value data buffer (only first `len` bytes are valid)
    data: [u8; MAX_VALUE_SIZE],
}

// Compare only initialized bytes (length-prefixed buffer pattern).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len.hash(state);
        self.as_bytes().hash(state);
    }
}

impl Value {
    /// Creates a new Value from byte slice
    ///
    /// # Errors
    /// Returns `HkvError::ValueTooLong` if data exceeds MAX_VALUE_SIZE
    pub fn new(data: &[u8]) -> HkvResult<Self> {
        if data.len() > MAX_VALUE_SIZE {
            return Err(HkvError::ValueTooLong);
        }

        let mut value = Value {
            len: data.len() as u16,
            data: [0u8; MAX_VALUE_SIZE],
        };
        value.data[..data.len()].copy_from_slice(data);
        Ok(value)
    }

    /// Returns the valid value data as a slice
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }

    /// Returns the value length
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns true if value is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.len() <= 32 {
            write!(f, "Value({:?})", String::from_utf8_lossy(self.as_bytes()))
        } else {
            write!(f, "Value({}B)", self.len())
        }
    }
}

/// Version number for optimistic concurrency control
///
/// Monotonically increasing counter updated on every write.
/// Used for:
/// - Detecting stale reads (version mismatch)
/// - Write-through invalidation protocol
/// - Bounded staleness (version delta threshold)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u64);

impl Version {
    /// Initial version for new entries
    pub const ZERO: Version = Version(0);

    /// Creates a new version
    #[inline]
    pub const fn new(v: u64) -> Self {
        Version(v)
    }

    /// Returns the version number
    #[inline]
    pub const fn get(&self) -> u64 {
        self.0
    }

    /// Increments the version and returns the new value
    #[inline]
    pub fn increment(&mut self) -> Version {
        self.0 = self.0.wrapping_add(1);
        *self
    }

    /// Returns the next version without modifying self
    #[inline]
    pub const fn next(&self) -> Version {
        Version(self.0.wrapping_add(1))
    }
}

/// Time-to-live for cache entries
///
/// Uses nanoseconds since CLOCK_MONOTONIC (not wall clock) to avoid issues
/// with time adjustments (NTP, DST, timezone changes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ttl(pub u64);

impl Ttl {
    /// No expiration (infinite TTL)
    pub const INFINITE: Ttl = Ttl(u64::MAX);

    /// Creates TTL from nanoseconds since boot
    #[inline]
    pub const fn from_nanos(nanos: u64) -> Self {
        Ttl(nanos)
    }

    /// Creates TTL from duration from now
    #[inline]
    pub fn from_duration(duration: Duration) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);

        Ttl((now + duration).as_nanos() as u64)
    }

    /// Returns nanoseconds value
    #[inline]
    pub const fn as_nanos(&self) -> u64 {
        self.0
    }

    /// Returns true if this TTL represents infinite (no expiration)
    #[inline]
    pub const fn is_infinite(&self) -> bool {
        self.0 == u64::MAX
    }

    /// Returns true if entry has expired relative to current time
    pub fn is_expired(&self, current_nanos: u64) -> bool {
        !self.is_infinite() && current_nanos >= self.0
    }
}

/// Entry flags bitfield
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFlags(pub u8);

impl EntryFlags {
    /// Entry is valid and can be read
    pub const VALID: u8 = 0b0000_0001;

    /// Entry is marked for eviction (pending removal)
    pub const EVICTING: u8 = 0b0000_0010;

    /// Entry has been invalidated by a write
    pub const INVALIDATED: u8 = 0b0000_0100;

    /// Creates empty flags
    #[inline]
    pub const fn empty() -> Self {
        EntryFlags(0)
    }

    /// Creates flags with VALID bit set
    #[inline]
    pub const fn valid() -> Self {
        EntryFlags(Self::VALID)
    }

    /// Returns true if VALID flag is set
    #[inline]
    pub const fn is_valid(&self) -> bool {
        (self.0 & Self::VALID) != 0
    }

    /// Returns true if EVICTING flag is set
    #[inline]
    pub const fn is_evicting(&self) -> bool {
        (self.0 & Self::EVICTING) != 0
    }

    /// Returns true if INVALIDATED flag is set
    #[inline]
    pub const fn is_invalidated(&self) -> bool {
        (self.0 & Self::INVALIDATED) != 0
    }

    /// Sets a flag bit
    #[inline]
    pub fn set(&mut self, flag: u8) {
        self.0 |= flag;
    }

    /// Clears a flag bit
    #[inline]
    pub fn clear(&mut self, flag: u8) {
        self.0 &= !flag;
    }
}

/// Entry metadata (without key/value data)
///
/// Used for statistics, eviction decisions, and consistency checks
/// without copying the full entry.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryMetadata {
    /// Version number for consistency
    pub version: Version,

    /// Time-to-live expiration
    pub ttl: Ttl,

    /// Creation timestamp (nanoseconds)
    pub created_at: u64,

    /// Last access timestamp (nanoseconds)
    pub accessed_at: u64,

    /// Entry flags
    pub flags: EntryFlags,

    /// Key length (for validation)
    pub key_len: u16,

    /// Value length (for validation)
    pub value_len: u16,
}

impl EntryMetadata {
    /// Creates new metadata with current timestamp
    pub fn new(version: Version, ttl: Ttl, key_len: u16, value_len: u16) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;

        EntryMetadata {
            version,
            ttl,
            created_at: now,
            accessed_at: now,
            flags: EntryFlags::valid(),
            key_len,
            value_len,
        }
    }

    /// Updates the access timestamp
    #[inline]
    pub fn touch(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;
        self.accessed_at = now;
    }

    /// Returns true if entry is expired
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;
        self.ttl.is_expired(now)
    }

    /// Returns entry age in nanoseconds
    pub fn age_nanos(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos() as u64;
        now.saturating_sub(self.created_at)
    }
}

/// Complete cache entry with key, value, and metadata
///
/// Total size: ~1328 bytes (includes alignment padding)
/// - Key: 258 bytes (2B len + 256B data)
/// - Value: 1026 bytes (2B len + 1024B data)
/// - Metadata: 40 bytes
#[repr(C)]
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    /// Key data
    pub key: Key,

    /// Value data
    pub value: Value,

    /// Entry metadata
    pub metadata: EntryMetadata,
}

impl Entry {
    /// Creates a new entry with current timestamp
    pub fn new(key: Key, value: Value, version: Version, ttl: Ttl) -> Self {
        let metadata = EntryMetadata::new(
            version,
            ttl,
            key.len() as u16,
            value.len() as u16,
        );

        Entry {
            key,
            value,
            metadata,
        }
    }

    /// Returns true if entry is valid (not expired, not invalidated)
    pub fn is_valid(&self) -> bool {
        self.metadata.flags.is_valid()
            && !self.metadata.is_expired()
            && !self.metadata.flags.is_invalidated()
    }

    /// Marks entry as accessed (updates access timestamp)
    #[inline]
    pub fn touch(&mut self) {
        self.metadata.touch();
    }

    /// Returns entry size in bytes (key + value + metadata)
    pub fn size(&self) -> usize {
        std::mem::size_of::<Entry>()
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("key", &self.key)
            .field("value", &self.value)
            .field("version", &self.metadata.version)
            .field("ttl", &self.metadata.ttl)
            .field("flags", &self.metadata.flags)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HkvError;

    #[test]
    fn test_key_creation() {
        let data = b"test_key";
        let key = Key::new(data).unwrap();
        assert_eq!(key.as_bytes(), data);
        assert_eq!(key.len(), 8);
        assert!(!key.is_empty());
    }

    #[test]
    fn test_key_max_size() {
        let data = vec![b'x'; MAX_KEY_SIZE];
        let key = Key::new(&data).unwrap();
        assert_eq!(key.len(), MAX_KEY_SIZE);

        // Exceeds max size
        let data = vec![b'x'; MAX_KEY_SIZE + 1];
        assert!(matches!(Key::new(&data), Err(HkvError::KeyTooLong)));
    }

    #[test]
    fn test_value_creation() {
        let data = b"test_value";
        let value = Value::new(data).unwrap();
        assert_eq!(value.as_bytes(), data);
        assert_eq!(value.len(), 10);
        assert!(!value.is_empty());
    }

    #[test]
    fn test_value_max_size() {
        let data = vec![b'x'; MAX_VALUE_SIZE];
        let value = Value::new(&data).unwrap();
        assert_eq!(value.len(), MAX_VALUE_SIZE);

        // Exceeds max size
        let data = vec![b'x'; MAX_VALUE_SIZE + 1];
        assert!(matches!(Value::new(&data), Err(HkvError::ValueTooLong)));
    }

    #[test]
    fn test_version() {
        let mut v = Version::ZERO;
        assert_eq!(v.get(), 0);

        v.increment();
        assert_eq!(v.get(), 1);

        let next = v.next();
        assert_eq!(next.get(), 2);
        assert_eq!(v.get(), 1); // Original unchanged
    }

    #[test]
    fn test_ttl() {
        let ttl = Ttl::INFINITE;
        assert!(ttl.is_infinite());
        assert!(!ttl.is_expired(u64::MAX - 1));

        let ttl = Ttl::from_nanos(1000);
        assert!(!ttl.is_infinite());
        assert!(ttl.is_expired(1001));
        assert!(!ttl.is_expired(999));
    }

    #[test]
    fn test_entry_flags() {
        let mut flags = EntryFlags::empty();
        assert!(!flags.is_valid());

        flags.set(EntryFlags::VALID);
        assert!(flags.is_valid());

        flags.set(EntryFlags::INVALIDATED);
        assert!(flags.is_invalidated());
        assert!(flags.is_valid()); // Both can be set

        flags.clear(EntryFlags::VALID);
        assert!(!flags.is_valid());
        assert!(flags.is_invalidated());
    }

    #[test]
    fn test_entry_creation() {
        let key = Key::new(b"key1").unwrap();
        let value = Value::new(b"value1").unwrap();
        let entry = Entry::new(key.clone(), value.clone(), Version::ZERO, Ttl::INFINITE);

        assert_eq!(entry.key, key);
        assert_eq!(entry.value, value);
        assert_eq!(entry.metadata.version, Version::ZERO);
        assert!(entry.is_valid());
    }

    #[test]
    fn test_entry_metadata() {
        let mut metadata = EntryMetadata::new(
            Version::new(5),
            Ttl::INFINITE,
            10,
            20,
        );

        assert_eq!(metadata.version.get(), 5);
        assert_eq!(metadata.key_len, 10);
        assert_eq!(metadata.value_len, 20);
        assert!(metadata.flags.is_valid());

        let accessed_before = metadata.accessed_at;
        std::thread::sleep(Duration::from_millis(1));
        metadata.touch();
        assert!(metadata.accessed_at > accessed_before);
    }

    #[test]
    fn test_entry_size() {
        let key = Key::new(b"k").unwrap();
        let value = Value::new(b"v").unwrap();
        let entry = Entry::new(key, value, Version::ZERO, Ttl::INFINITE);

        // Should be 1328 bytes as documented on 64-bit targets.
        let size = entry.size();
        assert_eq!(size, 1328);
    }

    #[test]
    fn test_struct_sizes() {
        assert_eq!(std::mem::size_of::<Key>(), 258);
        assert_eq!(std::mem::size_of::<Value>(), 1026);
        assert_eq!(std::mem::size_of::<EntryMetadata>(), 40);
        assert_eq!(std::mem::size_of::<Entry>(), 1328);
    }
}
