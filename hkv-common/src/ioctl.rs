// ioctl.rs - ioctl command definitions for HybridKV kernel module
//
// This module defines the ioctl commands used to communicate with the
// /dev/hybridkv character device.
//
// ============================================================================
// WHY IOCTL?
// ============================================================================
//
// ioctl (input/output control) is a Linux system call that allows user-space
// programs to communicate with device drivers and kernel modules. It's the
// standard mechanism for sending commands and control data to drivers.
//
// For HybridKV, we use ioctl because:
//
// 1. **Low Latency**: Direct syscall to kernel, minimal overhead (~100-200ns)
//    - No socket/network stack overhead
//    - No context switching between multiple processes
//    - Direct memory copy between user/kernel space
//
// 2. **Synchronous Operation**: ioctl blocks until kernel completes the request
//    - Simplifies error handling (immediate success/failure)
//    - Predictable latency characteristics
//    - No async callback complexity for fast operations
//
// 3. **Type Safety**: Can pass structured data with known sizes
//    - Request/response structs validated at compile time
//    - Kernel can verify magic numbers and sizes
//    - Reduces risk of memory corruption
//
// 4. **Standard Linux Pattern**: Well-understood by kernel developers
//    - Extensive documentation and examples
//    - Built-in support in kernel APIs
//    - Integration with existing tools (strace, etc.)
//
// Alternative approaches we considered but rejected:
//
// - **Netlink sockets**: Good for async notifications (we use this for eviction
//   events), but overkill for synchronous read/write operations. Higher overhead.
//
// - **procfs/sysfs**: Good for simple config values, but awkward for binary data
//   and structured operations. String parsing overhead unacceptable.
//
// - **Shared memory**: Extremely fast, but requires complex synchronization
//   (locks, atomics) and is harder to make safe. Risk of race conditions.
//
// - **System calls**: Would require patching the kernel with custom syscalls,
//   making it impossible to deploy as a loadable module.
//
// ============================================================================
// HOW IOCTL WORKS
// ============================================================================
//
// Communication flow:
//
// 1. **User Space** (hkv-client):
//    ```
//    fd = open("/dev/hybridkv", O_RDWR);  // Open device file
//    request = ReadRequest { key: "foo" };
//    result = ioctl(fd, CMD_READ, &request);  // Send command
//    if (result == 0) {
//        // Success: response written to request struct
//        value = request.response.value;
//    }
//    ```
//
// 2. **Kernel Transition**:
//    - CPU switches from user mode to kernel mode (context switch)
//    - Kernel validates the ioctl command number and permissions
//    - Kernel calls our driver's ioctl handler function
//
// 3. **Kernel Space** (hkv-kernel module):
//    ```
//    fn hybridkv_ioctl(fd, cmd, arg) {
//        match cmd {
//            CMD_READ => {
//                request = copy_from_user(arg);  // Copy request from user space
//                value = hash_table_lookup(request.key);
//                response = ReadResponse { value };
//                copy_to_user(arg, response);  // Copy response back to user
//                return 0;  // Success
//            }
//            _ => return -EINVAL;  // Invalid command
//        }
//    }
//    ```
//
// 4. **Return to User Space**:
//    - Kernel copies response data back to user space
//    - CPU switches back to user mode
//    - ioctl() returns 0 (success) or -1 (error with errno set)
//    - User space processes the response
//
// ============================================================================
// IOCTL COMMAND NUMBER ENCODING
// ============================================================================
//
// Linux ioctl command numbers are typically 32-bit values encoded as:
//
//   bits 31-30: Direction (read/write/both)
//   bits 29-16: Size of data structure
//   bits 15-8:  Magic number (unique per driver, 'H' for HybridKV)
//   bits 7-0:   Command number (0-255)
//
// However, for simplicity, we use just the command number (0-255) and handle
// the full encoding in the hkv-client crate using Linux's ioctl macros:
//
//   _IO(magic, nr)         - No data transfer
//   _IOR(magic, nr, type)  - Read from kernel
//   _IOW(magic, nr, type)  - Write to kernel
//   _IOWR(magic, nr, type) - Read and write
//
// Example:
//   CMD_READ is encoded as _IOWR('H', 0, ReadRequest)
//   This tells the kernel: "HybridKV command 0, bidirectional data transfer"
//
// ============================================================================
// SAFETY CONSIDERATIONS
// ============================================================================
//
// ioctl crosses the user/kernel boundary, which requires extreme care:
//
// 1. **Validate ALL user input**: Never trust data from user space
//    - Check sizes before copying (prevent buffer overflows)
//    - Validate magic numbers (detect corruption)
//    - Check version numbers (prevent ABI mismatches)
//
// 2. **Use copy_from_user/copy_to_user**: Never dereference user pointers directly
//    - These functions handle page faults safely
//    - Validate that user addresses are valid and accessible
//    - Prevent kernel crashes from bad pointers
//
// 3. **Bounds checking**: Enforce maximum sizes for keys/values
//    - MAX_KEY_SIZE = 256 bytes
//    - MAX_VALUE_SIZE = 1024 bytes
//    - Reject oversized requests before allocation
//
// 4. **Error handling**: Always return proper error codes
//    - Use standard Linux errno values (EINVAL, ENOMEM, etc.)
//    - Never panic or cause kernel oops
//    - Provide meaningful error context to user space
//
// 5. **Concurrency**: Multiple threads may call ioctl simultaneously
//    - Use proper locking (RCU for reads, spinlocks for writes)
//    - Avoid holding locks during copy_to_user (may page fault)
//    - Design for lock-free reads where possible
//
// ============================================================================
// PERFORMANCE CHARACTERISTICS
// ============================================================================
//
// Typical latencies (on modern x86_64 CPU):
//
// - ioctl syscall overhead: ~100-200ns
// - copy_from_user (256 bytes): ~50-100ns
// - Hash table lookup (RCU): ~20-50ns
// - copy_to_user (1KB): ~100-200ns
// - Total READ latency: ~300-600ns (best case)
//
// With kernel cache hit: 1-5μs end-to-end
// Without kernel (user-space only): 15-30μs
//
// Speedup: ~5-10x for hot keys
//
// ============================================================================
// DESIGN NOTES
// ============================================================================
//
// - Each command has a unique number (0-255)
// - Commands follow Linux ioctl conventions
// - All commands go through the /dev/hybridkv device file
// - Magic number 'H' (0x48) identifies HybridKV commands
// - Commands are grouped logically: data ops (0-4), monitoring (5), control (6-7)

/// ioctl magic number for HybridKV device
///
/// This is 'H' (0x48) in ASCII, representing "HybridKV"
/// Each device driver should have a unique magic number to avoid conflicts
pub const IOCTL_MAGIC: u8 = b'H';

/// Device file path for kernel interface
pub const DEVICE_PATH: &str = "/dev/hybridkv";

/// Device name (as shown in /proc/devices)
pub const DEVICE_NAME: &str = "hybridkv";

// ============================================================================
// IOCTL COMMAND NUMBERS
// ============================================================================

/// Command number for READ operation
///
/// Fast path: Read a value from kernel cache
/// - Input: Key (byte array)
/// - Output: Value (byte array) if found, or KeyNotFound error
/// - Expected latency: 1-5μs for cache hit
///
/// This is the most performance-critical operation. If the key is cached
/// in kernel space, it returns immediately with minimal overhead.
pub const CMD_READ: u8 = 0;

/// Command number for PROMOTE operation
///
/// Add a single key-value pair to kernel cache
/// - Input: Key + Value + Metadata (version, TTL)
/// - Output: Success or error (NoMemory, KeyTooLarge, etc.)
///
/// User space decides which keys to promote based on access patterns.
/// Kernel accepts the entry and stores it in the hash table.
pub const CMD_PROMOTE: u8 = 1;

/// Command number for BATCH_PROMOTE operation
///
/// Promote multiple key-value pairs in a single ioctl call
/// - Input: Array of (Key, Value, Metadata) entries
/// - Output: Per-entry success/failure bitmap
/// - Maximum: 1000 entries per batch
///
/// This is more efficient than 1000 individual PROMOTE calls because
/// it amortizes the syscall overhead across all entries.
pub const CMD_BATCH_PROMOTE: u8 = 2;

/// Command number for DEMOTE operation
///
/// Remove a key from kernel cache
/// - Input: Key
/// - Output: Success (even if key wasn't cached)
///
/// User space may demote keys that are no longer hot, or when
/// explicitly deleting a key from the storage.
pub const CMD_DEMOTE: u8 = 3;

/// Command number for INVALIDATE operation
///
/// Mark a cached entry as stale (due to user-space write)
/// - Input: Key + New version number
/// - Output: Success
///
/// When user space updates a value, it must invalidate the kernel cache.
/// The kernel marks the entry as stale but doesn't delete it immediately
/// (grace period). Next read will return STALE, prompting re-promotion
/// if the key is still hot.
///
/// This is critical for maintaining consistency in the write-through
/// cache model.
pub const CMD_INVALIDATE: u8 = 4;

/// Command number for STATS operation
///
/// Get kernel cache statistics
/// - Input: None
/// - Output: Statistics struct (memory usage, hit/miss rates, etc.)
///
/// Returns comprehensive statistics about cache state:
/// - Memory usage (current, peak, limit)
/// - Entry count (current, peak, limit)
/// - Operation counts (hits, misses, promotions, evictions)
/// - Performance metrics (lock contentions, RCU grace periods)
pub const CMD_STATS: u8 = 5;

/// Command number for CONFIG operation
///
/// Update kernel cache runtime configuration
/// - Input: Configuration struct
/// - Output: Success or validation error
///
/// Allows runtime tuning of:
/// - Eviction strategy (LRU, LFU, FIFO)
/// - Memory limits
/// - Entry limits
/// - Enable/disable cache
///
/// Some config changes may require draining the cache.
pub const CMD_CONFIG: u8 = 6;

/// Command number for FLUSH operation
///
/// Clear all entries from kernel cache
/// - Input: None
/// - Output: Success
///
/// This is used for:
/// - Testing and debugging
/// - Emergency recovery (if cache is misbehaving)
/// - Graceful shutdown before unloading module
///
/// Warning: This is expensive! Must wait for RCU grace period to
/// ensure no readers are accessing the entries being freed.
pub const CMD_FLUSH: u8 = 7;

// ============================================================================
// COMMAND ENUMERATION
// ============================================================================

/// All available ioctl commands
///
/// This enum provides a type-safe way to work with command numbers.
/// Each variant corresponds to one of the CMD_* constants above.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoctlCommand {
    /// Read value from kernel cache (fast path)
    Read = CMD_READ,

    /// Promote single entry to kernel cache
    Promote = CMD_PROMOTE,

    /// Promote multiple entries in one call (batch)
    BatchPromote = CMD_BATCH_PROMOTE,

    /// Remove entry from kernel cache
    Demote = CMD_DEMOTE,

    /// Mark entry as stale (on write)
    Invalidate = CMD_INVALIDATE,

    /// Get cache statistics
    Stats = CMD_STATS,

    /// Update runtime configuration
    Config = CMD_CONFIG,

    /// Flush all entries from cache
    Flush = CMD_FLUSH,
}

impl IoctlCommand {
    /// Convert command to u8 number
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Try to create command from u8 number
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            CMD_READ => Some(Self::Read),
            CMD_PROMOTE => Some(Self::Promote),
            CMD_BATCH_PROMOTE => Some(Self::BatchPromote),
            CMD_DEMOTE => Some(Self::Demote),
            CMD_INVALIDATE => Some(Self::Invalidate),
            CMD_STATS => Some(Self::Stats),
            CMD_CONFIG => Some(Self::Config),
            CMD_FLUSH => Some(Self::Flush),
            _ => None,
        }
    }

    /// Get human-readable command name
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "READ",
            Self::Promote => "PROMOTE",
            Self::BatchPromote => "BATCH_PROMOTE",
            Self::Demote => "DEMOTE",
            Self::Invalidate => "INVALIDATE",
            Self::Stats => "STATS",
            Self::Config => "CONFIG",
            Self::Flush => "FLUSH",
        }
    }

    /// Check if command is read-only (doesn't modify cache)
    pub const fn is_readonly(self) -> bool {
        matches!(self, Self::Read | Self::Stats)
    }

    /// Check if command modifies cache
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::Promote | Self::BatchPromote | Self::Demote | Self::Invalidate | Self::Flush
        )
    }

    /// Check if command is a configuration operation
    pub const fn is_config(self) -> bool {
        matches!(self, Self::Config)
    }
}

impl std::fmt::Display for IoctlCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_conversion() {
        // Test all commands can be converted to/from u8
        let commands = [
            IoctlCommand::Read,
            IoctlCommand::Promote,
            IoctlCommand::BatchPromote,
            IoctlCommand::Demote,
            IoctlCommand::Invalidate,
            IoctlCommand::Stats,
            IoctlCommand::Config,
            IoctlCommand::Flush,
        ];

        for cmd in commands {
            let num = cmd.as_u8();
            let back = IoctlCommand::from_u8(num);
            assert_eq!(Some(cmd), back);
        }
    }

    #[test]
    fn test_invalid_command() {
        // Invalid command number should return None
        assert_eq!(IoctlCommand::from_u8(255), None);
        assert_eq!(IoctlCommand::from_u8(99), None);
    }

    #[test]
    fn test_command_classification() {
        // Read operations
        assert!(IoctlCommand::Read.is_readonly());
        assert!(IoctlCommand::Stats.is_readonly());
        assert!(!IoctlCommand::Read.is_write());

        // Write operations
        assert!(IoctlCommand::Promote.is_write());
        assert!(IoctlCommand::Demote.is_write());
        assert!(IoctlCommand::Invalidate.is_write());
        assert!(IoctlCommand::Flush.is_write());
        assert!(!IoctlCommand::Promote.is_readonly());

        // Config operations
        assert!(IoctlCommand::Config.is_config());
        assert!(!IoctlCommand::Read.is_config());
    }

    #[test]
    fn test_command_names() {
        assert_eq!(IoctlCommand::Read.name(), "READ");
        assert_eq!(IoctlCommand::Promote.name(), "PROMOTE");
        assert_eq!(IoctlCommand::BatchPromote.name(), "BATCH_PROMOTE");
    }

    #[test]
    fn test_command_display() {
        let cmd = IoctlCommand::Read;
        assert_eq!(format!("{}", cmd), "READ");
    }

    #[test]
    fn test_magic_number() {
        // Magic number should be 'H' in ASCII
        assert_eq!(IOCTL_MAGIC, b'H');
        assert_eq!(IOCTL_MAGIC, 0x48);
    }

    #[test]
    fn test_command_uniqueness() {
        // All command numbers should be unique
        let numbers = [
            CMD_READ,
            CMD_PROMOTE,
            CMD_BATCH_PROMOTE,
            CMD_DEMOTE,
            CMD_INVALIDATE,
            CMD_STATS,
            CMD_CONFIG,
            CMD_FLUSH,
        ];

        for i in 0..numbers.len() {
            for j in (i + 1)..numbers.len() {
                assert_ne!(
                    numbers[i], numbers[j],
                    "Command {} conflicts with command {}",
                    i, j
                );
            }
        }
    }
}
