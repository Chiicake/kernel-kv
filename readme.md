# HybridKV

A high-performance key-value storage engine that combines user-space flexibility with kernel-space speed. HybridKV eliminates mode-switch overhead for hot data by caching frequently accessed keys in a kernel module, while maintaining full functionality in user space.

## Goal of HybridKV

**Performance Gains**
- **1-5μs latency** for hot key reads (vs 15-30μs in pure user-space)
- **50-80% reduction** in P99 latency for hot workloads
- **Zero-copy** kernel cache with RCU lock-free reads

**Smart Hybrid Design**
- Automatic hot data tracking and promotion to kernel space
- Seamless fallback to user space (zero kernel panics)
- Pluggable eviction strategies (LRU/LFU/FIFO)
- Redis RESP protocol compatible

**Others**
- Write-through consistency (kernel is read-only cache)
- Comprehensive observability with web GUI
- Safe kernel primitives (no system crashes)
- AOF/RDB persistence support

## Architecture

HybridKV is built as a Cargo workspace with six modules:

```
hybridkv/
├── hkv-engine/    # Core storage engine (String, List, Hash, Set, ZSet)
├── hkv-server/    # Network server with hot data tracking
├── hkv-client/    # Client library with smart routing
├── hkv-kernel/    # Kernel module (read-only hot key cache)
├── hkv-common/    # Shared types and protocols
└── hkv-gui/       # Web-based management dashboard
```

```
┌──────────────────────────────────────────────────────────────┐
│                   Client Application                          │
└────────────────────────┬─────────────────────────────────────┘
                         │
              ┌──────────┴──────────┐
              │  hkv-client (lib)   │
              │  ┌───────────────┐  │
              │  │ Kernel First? │  │  (checks /dev/hybridkv)
              │  └───────┬───────┘  │
              └──────────┼──────────┘
                         │
        ┌────────────────┴─────────────────┐
        │ Kernel Hit                       │ Kernel Miss / Fallback
        │                                  │
┌───────▼──────────┐              ┌────────▼────────────┐
│  Kernel Space    │              │   User Space        │
│  (hkv-kernel)    │              │   (hkv-server)      │
│                  │              │                     │
│ ┌──────────────┐ │              │ ┌─────────────────┐│
│ │ RCU Hash     │ │    ioctl     │ │  hkv-engine     ││
│ │ Table        │ │◄────sync─────┤ │  (Full Store)   ││
│ └──────────────┘ │              │ └─────────────────┘│
│ ┌──────────────┐ │              │ ┌─────────────────┐│
│ │ Eviction:    │ │   eviction   │ │ Hot Tracker     ││
│ │ LRU/LFU/FIFO │─┼─────notify──►│ │ (Statistics)    ││
│ └──────────────┘ │              │ └─────────────────┘│
│                  │              │ ┌─────────────────┐│
│                  │   promote/   │ │ Promotion Mgr   ││
│                  │◄──demote─────┤ │ (Bg Thread)     ││
│                  │              │ └─────────────────┘│
└──────────────────┘              └─────────────────────┘
         │                                  │
         └──────────────┬───────────────────┘
                        │
                 ┌──────▼───────┐
                 │/dev/hybridkv │
                 │(Char Device) │
                 └──────────────┘
```

**How It Works**
1. All writes go to user-space storage engine (hkv-engine)
2. Background thread tracks hot keys and promotes to kernel (hkv-kernel)
3. Reads check kernel cache first, fall back to user space
4. Kernel autonomously evicts cold keys based on configured strategy

## Tech Stack

**User Space**
- Rust (safe systems programming)
- Tokio (async runtime)
- Redis RESP protocol
- Count-Min Sketch (hot key tracking)
- React + TypeScript (GUI)

**Kernel Space**
- Rust for Linux (Linux 6.1+)
- RCU hash tables (lock-free reads)
- Netlink sockets (async notifications)
- Character device (`/dev/hybridkv`)

**Communication**
- ioctl (fast path for cache operations)
- Netlink (async kernel-to-user notifications)
- Shared memory regions (zero-copy data transfer)

## Documentation

Still working...

## Status

**In Development** - Currently in design phase. Contributions and feedback welcome!

