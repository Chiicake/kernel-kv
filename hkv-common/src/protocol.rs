//! # Protocol Structures
//!
//! Purpose: Define FFI-safe request/response headers for ioctl communication.
//!
//! ## Design Principles
//!
//! 1. **FFI Stability**: Use `#[repr(C)]` to keep user/kernel layouts consistent.
//! 2. **Minimal Overhead**: Keep headers tiny to reduce copy and cache pressure.
//! 3. **Versioned ABI**: Embed a protocol version for forward compatibility checks.
//!
//! ## Usage Notes
//!
//! - Request types carry the command in `IoctlHeader` so the kernel can validate
//!   metadata before touching payloads.
//! - Response types return `STATUS_OK` on success or `HkvError::code()` on failure.
//! - Fixed-size buffers keep the ABI stable even when payloads are partially filled.
//!
//! ## Memory Layout Example
//!
//! ```text
//! IoctlHeader (4 bytes total):
//! +--------+---------+----------+----------+
//! | magic  | version | command  | reserved |
//! +--------+---------+----------+----------+
//! | 1B     | 1B      | 1B       | 1B       |
//! +--------+---------+----------+----------+
//!
//! ReadRequest (262 bytes total):
//! +------------+---------+
//! | header:4B  | key:258B|
//! +------------+---------+
//!
//! ReadResponse (1032 bytes total):
//! +------------+-----------+-------------+
//! | header:4B  | status:2B | value:1026B |
//! +------------+-----------+-------------+
//!
//! PromoteRequest (1304 bytes total):
//! +------------+---------+-----------+-----------+--------+
//! | header:4B  | key:258B| value:1026B| version:8B| ttl:8B |
//! +------------+---------+-----------+-----------+--------+
//!
//! PromoteResponse (8 bytes total):
//! +------------+-----------+-------------+
//! | header:4B  | status:2B | reserved:2B |
//! +------------+-----------+-------------+
//!
//! BatchPromoteRequest (1304008 bytes total):
//! +------------+----------+------------+-----------------------+
//! | header:4B  | count:2B | reserved:2B| entries:1304000B      |
//! +------------+----------+------------+-----------------------+
//!
//! BatchPromoteResponse (134 bytes total):
//! +------------+----------+------------+-----------------------+
//! | header:4B  | count:2B | reserved:2B| results:125B          |
//! +------------+----------+------------+-----------------------+
//!
//! DemoteRequest (262 bytes total):
//! +------------+---------+
//! | header:4B  | key:258B|
//! +------------+---------+
//!
//! InvalidateRequest (272 bytes total):
//! +------------+---------+-----------+
//! | header:4B  | key:258B| pad:2B    |
//! +------------+---------+-----------+
//! | version:8B                       |
//! +----------------------------------+
//!
//! StatsRequest (4 bytes total):
//! +------------+
//! | header:4B  |
//! +------------+
//!
//! StatsResponse (112 bytes total):
//! +------------+-----------+-------------+-------------------+
//! | header:4B  | status:2B | reserved:2B | stats:104B        |
//! +------------+-----------+-------------+-------------------+
//!
//! ConfigRequest (40 bytes total):
//! +------------+-------------+-------------+-----------+-----------+
//! | header:4B  | pad:4B      | max_bytes:8B| max_entries:8B        |
//! +------------+-------------+-------------+-----------+-----------+
//! | high:4B    | low:4B      | reserved:8B                           |
//! +------------+-------------+---------------------------------------+
//!
//! FlushRequest (4 bytes total):
//! +------------+
//! | header:4B  |
//! +------------+
//! ```

use crate::ioctl::{IoctlCommand, IOCTL_MAGIC};
use crate::types::{Key, Ttl, Value, Version};

/// Protocol version for user/kernel ABI compatibility.
pub const PROTOCOL_VERSION: u8 = 1;

/// Status code indicating success in ioctl responses.
pub const STATUS_OK: u16 = 0;

/// Maximum number of entries in a batch promote request.
pub const MAX_BATCH_SIZE: usize = 1000;

/// Result bitmap size for batch responses (1 bit per entry).
pub const BATCH_RESULT_BYTES: usize = (MAX_BATCH_SIZE + 7) / 8;

/// Common header prepended to ioctl request/response payloads.
///
/// This header is `repr(C)` to preserve C ABI layout for kernel interop.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IoctlHeader {
    /// Magic number to validate the device protocol.
    pub magic: u8,
    /// Protocol version for ABI checks.
    pub version: u8,
    /// Command number describing the request.
    pub command: u8,
    /// Reserved for alignment and future flags; must be zero.
    pub reserved: u8,
}

impl IoctlHeader {
    /// Builds a header for the provided ioctl command.
    pub const fn new(command: IoctlCommand) -> Self {
        IoctlHeader {
            magic: IOCTL_MAGIC,
            version: PROTOCOL_VERSION,
            command: command.as_u8(),
            reserved: 0,
        }
    }
}

/// Read request payload for a cache lookup.
///
/// Uses the header + payload pattern to validate command metadata once and
/// keep the key inline for zero-allocation FFI transfers.
///
/// Use: Issued by user space to fetch a value from the kernel cache.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRequest {
    /// Common ioctl header (command must be READ).
    pub header: IoctlHeader,
    /// Lookup key (length-prefixed, fixed-capacity buffer).
    pub key: Key,
}

impl ReadRequest {
    /// Builds a read request for the provided key.
    pub fn new(key: Key) -> Self {
        ReadRequest {
            header: IoctlHeader::new(IoctlCommand::Read),
            key,
        }
    }
}

/// Read response payload for a cache lookup.
///
/// The `status` field uses `STATUS_OK` for success or an `HkvError::code()`
/// value on failure. The `value` buffer is valid only when `status` is OK.
///
/// Use: Returned by the kernel after processing a read lookup.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResponse {
    /// Common ioctl header (command must be READ).
    pub header: IoctlHeader,
    /// Status code (0 on success, error code on failure).
    pub status: u16,
    /// Value buffer (length-prefixed, fixed-capacity buffer).
    pub value: Value,
}

impl ReadResponse {
    /// Builds a read response with an explicit status and value.
    pub fn new(status: u16, value: Value) -> Self {
        ReadResponse {
            header: IoctlHeader::new(IoctlCommand::Read),
            status,
            value,
        }
    }
}

/// Promote request payload for inserting a single entry into the kernel cache.
///
/// The header identifies the command, while the payload carries only the
/// minimum metadata needed for cache admission (version + TTL) to keep the
/// user/kernel copy as small as possible.
///
/// Use: Issued by user space to promote one entry into the kernel cache.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromoteRequest {
    /// Common ioctl header (command must be PROMOTE).
    pub header: IoctlHeader,
    /// Entry key to insert.
    pub key: Key,
    /// Entry value to insert.
    pub value: Value,
    /// Version to associate with the entry.
    pub version: Version,
    /// Absolute expiration timestamp for the entry.
    pub ttl: Ttl,
}

impl PromoteRequest {
    /// Builds a promote request for the provided entry data.
    pub fn new(key: Key, value: Value, version: Version, ttl: Ttl) -> Self {
        PromoteRequest {
            header: IoctlHeader::new(IoctlCommand::Promote),
            key,
            value,
            version,
            ttl,
        }
    }
}

/// Promote response payload indicating success or failure.
///
/// Uses `STATUS_OK` on success or an `HkvError::code()` value on failure.
///
/// Use: Returned by the kernel after handling a promote request.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromoteResponse {
    /// Common ioctl header (command must be PROMOTE).
    pub header: IoctlHeader,
    /// Status code (0 on success, error code on failure).
    pub status: u16,
    /// Reserved for future flags; must be zero.
    pub reserved: u16,
}

impl PromoteResponse {
    /// Builds a promote response with an explicit status.
    pub fn new(status: u16) -> Self {
        PromoteResponse {
            header: IoctlHeader::new(IoctlCommand::Promote),
            status,
            reserved: 0,
        }
    }
}

/// Single batch promote entry (payload-only).
///
/// This keeps the batch payload compact by avoiding per-entry headers.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchPromoteEntry {
    /// Entry key to insert.
    pub key: Key,
    /// Entry value to insert.
    pub value: Value,
    /// Version to associate with the entry.
    pub version: Version,
    /// Absolute expiration timestamp for the entry.
    pub ttl: Ttl,
}

impl BatchPromoteEntry {
    /// Builds a batch entry for the provided data.
    pub fn new(key: Key, value: Value, version: Version, ttl: Ttl) -> Self {
        BatchPromoteEntry {
            key,
            value,
            version,
            ttl,
        }
    }
}

/// Batch promote request payload for inserting multiple entries.
///
/// Uses the header+payload pattern to amortize syscall overhead while
/// preserving a flat, FFI-friendly layout.
///
/// Use: Issued by user space to promote multiple entries in one ioctl call.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchPromoteRequest {
    /// Common ioctl header (command must be BATCH_PROMOTE).
    pub header: IoctlHeader,
    /// Number of valid entries in the batch (<= MAX_BATCH_SIZE).
    pub count: u16,
    /// Reserved for alignment/future flags; must be zero.
    pub reserved: u16,
    /// Fixed-capacity entry array (only first `count` are valid).
    pub entries: [BatchPromoteEntry; MAX_BATCH_SIZE],
}

impl BatchPromoteRequest {
    /// Builds a batch promote request for the provided entries.
    pub fn new(entries: [BatchPromoteEntry; MAX_BATCH_SIZE], count: u16) -> Self {
        debug_assert!(count as usize <= MAX_BATCH_SIZE);
        BatchPromoteRequest {
            header: IoctlHeader::new(IoctlCommand::BatchPromote),
            count,
            reserved: 0,
            entries,
        }
    }
}

/// Batch promote response payload with per-entry success bitmap.
///
/// Uses a bitmap pattern: bit=1 indicates success, bit=0 indicates failure.
///
/// Use: Returned by the kernel to report batch promotion results.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchPromoteResponse {
    /// Common ioctl header (command must be BATCH_PROMOTE).
    pub header: IoctlHeader,
    /// Number of valid results (matches request count).
    pub count: u16,
    /// Reserved for alignment/future flags; must be zero.
    pub reserved: u16,
    /// Success bitmap (1 bit per entry, LSB-first within each byte).
    pub results: [u8; BATCH_RESULT_BYTES],
}

impl BatchPromoteResponse {
    /// Builds an empty batch promote response for the given count.
    pub fn new(count: u16) -> Self {
        debug_assert!(count as usize <= MAX_BATCH_SIZE);
        BatchPromoteResponse {
            header: IoctlHeader::new(IoctlCommand::BatchPromote),
            count,
            reserved: 0,
            results: [0u8; BATCH_RESULT_BYTES],
        }
    }
}

/// Demote request payload for removing an entry from the kernel cache.
///
/// Use: Issued by user space to remove a key from the kernel cache.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoteRequest {
    /// Common ioctl header (command must be DEMOTE).
    pub header: IoctlHeader,
    /// Entry key to remove.
    pub key: Key,
}

impl DemoteRequest {
    /// Builds a demote request for the provided key.
    pub fn new(key: Key) -> Self {
        DemoteRequest {
            header: IoctlHeader::new(IoctlCommand::Demote),
            key,
        }
    }
}

/// Invalidate request payload for marking a cached entry as stale.
///
/// Use: Issued by user space after a write to invalidate a cached key.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidateRequest {
    /// Common ioctl header (command must be INVALIDATE).
    pub header: IoctlHeader,
    /// Entry key to invalidate.
    pub key: Key,
    /// New version number for the entry.
    pub version: Version,
}

impl InvalidateRequest {
    /// Builds an invalidate request for the provided key and version.
    pub fn new(key: Key, version: Version) -> Self {
        InvalidateRequest {
            header: IoctlHeader::new(IoctlCommand::Invalidate),
            key,
            version,
        }
    }
}

/// Snapshot of kernel cache statistics for telemetry.
///
/// All fields are plain counters or gauges so user space can render telemetry
/// without extra parsing or allocations.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    /// Total lookup attempts.
    pub lookups: u64,
    /// Cache hits.
    pub hits: u64,
    /// Cache misses.
    pub misses: u64,
    /// Hits on stale entries.
    pub stale_hits: u64,
    /// Successful promotions into kernel cache.
    pub promotions: u64,
    /// Demotions from kernel cache.
    pub demotions: u64,
    /// Evictions due to policy or pressure.
    pub evictions: u64,
    /// Invalidations from user-space writes.
    pub invalidations: u64,
    /// Current cache memory usage in bytes.
    pub used_bytes: u64,
    /// Configured memory limit in bytes.
    pub max_bytes: u64,
    /// Current number of cached entries.
    pub entry_count: u64,
    /// Lock contention events in the kernel fast path.
    pub lock_contentions: u64,
    /// Completed RCU grace periods.
    pub rcu_grace_periods: u64,
}

/// Stats request payload for fetching kernel cache telemetry.
///
/// Use: Issued by user space to retrieve a snapshot of cache stats.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsRequest {
    /// Common ioctl header (command must be STATS).
    pub header: IoctlHeader,
}

impl StatsRequest {
    /// Builds a stats request.
    pub const fn new() -> Self {
        StatsRequest {
            header: IoctlHeader::new(IoctlCommand::Stats),
        }
    }
}

/// Stats response payload with a snapshot of cache telemetry.
///
/// Uses `STATUS_OK` on success or an `HkvError::code()` value on failure.
///
/// Use: Returned by the kernel with counters and gauges for telemetry.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsResponse {
    /// Common ioctl header (command must be STATS).
    pub header: IoctlHeader,
    /// Status code (0 on success, error code on failure).
    pub status: u16,
    /// Reserved for alignment/future flags; must be zero.
    pub reserved: u16,
    /// Snapshot of cache statistics.
    pub stats: CacheStats,
}

impl StatsResponse {
    /// Builds a stats response with the provided status and stats snapshot.
    pub fn new(status: u16, stats: CacheStats) -> Self {
        StatsResponse {
            header: IoctlHeader::new(IoctlCommand::Stats),
            status,
            reserved: 0,
            stats,
        }
    }
}

/// Runtime configuration update for the kernel cache.
///
/// This keeps configuration fields aligned and explicit for easy validation
/// inside the kernel module.
///
/// Use: Issued by user space to update cache limits and watermarks.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigRequest {
    /// Common ioctl header (command must be CONFIG).
    pub header: IoctlHeader,
    /// Maximum memory allowed for the cache (bytes).
    pub max_bytes: u64,
    /// Maximum number of entries allowed.
    pub max_entries: u64,
    /// High watermark percentage (0-100) for eviction start.
    pub high_watermark: u32,
    /// Low watermark percentage (0-100) for eviction stop.
    pub low_watermark: u32,
    /// Reserved for future configuration fields; must be zero.
    pub reserved: u64,
}

impl ConfigRequest {
    /// Builds a config request with explicit values.
    pub fn new(
        max_bytes: u64,
        max_entries: u64,
        high_watermark: u32,
        low_watermark: u32,
    ) -> Self {
        ConfigRequest {
            header: IoctlHeader::new(IoctlCommand::Config),
            max_bytes,
            max_entries,
            high_watermark,
            low_watermark,
            reserved: 0,
        }
    }
}

/// Flush request payload for clearing all kernel cache entries.
///
/// Use: Issued by user space to clear all kernel cache entries.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushRequest {
    /// Common ioctl header (command must be FLUSH).
    pub header: IoctlHeader,
}

impl FlushRequest {
    /// Builds a flush request.
    pub const fn new() -> Self {
        FlushRequest {
            header: IoctlHeader::new(IoctlCommand::Flush),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ioctl_header_new() {
        let header = IoctlHeader::new(IoctlCommand::Read);
        assert_eq!(header.magic, IOCTL_MAGIC);
        assert_eq!(header.version, PROTOCOL_VERSION);
        assert_eq!(header.command, IoctlCommand::Read.as_u8());
        assert_eq!(header.reserved, 0);
    }

    #[test]
    fn test_ioctl_header_size() {
        assert_eq!(std::mem::size_of::<IoctlHeader>(), 4);
    }

    #[test]
    fn test_read_request_new() {
        let key = Key::new(b"alpha").unwrap();
        let request = ReadRequest::new(key.clone());
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Read));
        assert_eq!(request.key, key);
    }

    #[test]
    fn test_read_response_new() {
        let value = Value::new(b"beta").unwrap();
        let response = ReadResponse::new(STATUS_OK, value.clone());
        assert_eq!(response.header, IoctlHeader::new(IoctlCommand::Read));
        assert_eq!(response.status, STATUS_OK);
        assert_eq!(response.value, value);
    }

    #[test]
    fn test_read_struct_sizes() {
        assert_eq!(std::mem::size_of::<ReadRequest>(), 262);
        assert_eq!(std::mem::size_of::<ReadResponse>(), 1032);
    }

    #[test]
    fn test_promote_request_new() {
        let key = Key::new(b"alpha").unwrap();
        let value = Value::new(b"beta").unwrap();
        let request = PromoteRequest::new(key.clone(), value.clone(), Version::ZERO, Ttl::INFINITE);
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Promote));
        assert_eq!(request.key, key);
        assert_eq!(request.value, value);
        assert_eq!(request.version, Version::ZERO);
        assert_eq!(request.ttl, Ttl::INFINITE);
    }

    #[test]
    fn test_promote_response_new() {
        let response = PromoteResponse::new(STATUS_OK);
        assert_eq!(response.header, IoctlHeader::new(IoctlCommand::Promote));
        assert_eq!(response.status, STATUS_OK);
        assert_eq!(response.reserved, 0);
    }

    #[test]
    fn test_promote_struct_sizes() {
        assert_eq!(std::mem::size_of::<PromoteRequest>(), 1304);
        assert_eq!(std::mem::size_of::<PromoteResponse>(), 8);
    }

    #[test]
    fn test_batch_promote_entry_size() {
        assert_eq!(std::mem::size_of::<BatchPromoteEntry>(), 1304);
    }

    #[test]
    fn test_batch_promote_response_new() {
        let response = BatchPromoteResponse::new(10);
        assert_eq!(response.header, IoctlHeader::new(IoctlCommand::BatchPromote));
        assert_eq!(response.count, 10);
        assert_eq!(response.reserved, 0);
        assert_eq!(response.results.len(), BATCH_RESULT_BYTES);
    }

    #[test]
    fn test_batch_promote_struct_sizes() {
        assert_eq!(std::mem::size_of::<BatchPromoteRequest>(), 1_304_008);
        assert_eq!(std::mem::size_of::<BatchPromoteResponse>(), 134);
    }

    #[test]
    fn test_demote_request_new() {
        let key = Key::new(b"alpha").unwrap();
        let request = DemoteRequest::new(key.clone());
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Demote));
        assert_eq!(request.key, key);
    }

    #[test]
    fn test_invalidate_request_new() {
        let key = Key::new(b"alpha").unwrap();
        let request = InvalidateRequest::new(key.clone(), Version::new(42));
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Invalidate));
        assert_eq!(request.key, key);
        assert_eq!(request.version, Version::new(42));
    }

    #[test]
    fn test_demote_invalidate_sizes() {
        assert_eq!(std::mem::size_of::<DemoteRequest>(), 262);
        assert_eq!(std::mem::size_of::<InvalidateRequest>(), 272);
    }

    #[test]
    fn test_stats_request_new() {
        let request = StatsRequest::new();
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Stats));
    }

    #[test]
    fn test_stats_response_new() {
        let stats = CacheStats {
            lookups: 1,
            hits: 2,
            misses: 3,
            stale_hits: 4,
            promotions: 5,
            demotions: 6,
            evictions: 7,
            invalidations: 8,
            used_bytes: 9,
            max_bytes: 10,
            entry_count: 11,
            lock_contentions: 12,
            rcu_grace_periods: 13,
        };
        let response = StatsResponse::new(STATUS_OK, stats);
        assert_eq!(response.header, IoctlHeader::new(IoctlCommand::Stats));
        assert_eq!(response.status, STATUS_OK);
        assert_eq!(response.reserved, 0);
        assert_eq!(response.stats, stats);
    }

    #[test]
    fn test_stats_struct_sizes() {
        assert_eq!(std::mem::size_of::<CacheStats>(), 104);
        assert_eq!(std::mem::size_of::<StatsRequest>(), 4);
        assert_eq!(std::mem::size_of::<StatsResponse>(), 112);
    }

    #[test]
    fn test_config_request_new() {
        let request = ConfigRequest::new(256, 100, 80, 70);
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Config));
        assert_eq!(request.max_bytes, 256);
        assert_eq!(request.max_entries, 100);
        assert_eq!(request.high_watermark, 80);
        assert_eq!(request.low_watermark, 70);
        assert_eq!(request.reserved, 0);
    }

    #[test]
    fn test_flush_request_new() {
        let request = FlushRequest::new();
        assert_eq!(request.header, IoctlHeader::new(IoctlCommand::Flush));
    }

    #[test]
    fn test_config_flush_sizes() {
        assert_eq!(std::mem::size_of::<ConfigRequest>(), 40);
        assert_eq!(std::mem::size_of::<FlushRequest>(), 4);
    }
}
