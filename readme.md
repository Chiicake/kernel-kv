# HybridKV

HybridKV is a two-tier key-value system that keeps the authoritative KV engine in user space while offering a kernel-resident hot-key cache for low-latency reads. Unlike single-purpose in-kernel caches, HybridKV frameworkizes the kernel cache mechanism: the kernel provides a general data plane (object storage, indexing, concurrency-optimized read path, invalidation/versioning, memory budgeting, telemetry), and all cache decision logic is implemented as pluggable policies, deliver fast read paths for highly skewed workloads without pushing full KV semantics into the kernel. 

[//]: # ()
[//]: # (## Why HybridKV)

[//]: # (### Key strengths)

[//]: # ()
[//]: # (- Frameworkized kernel cache mechanism: a reusable kernel data plane with stable extension points, enabling rapid policy iteration without rewriting the kernel core.)

[//]: # ()
[//]: # (- Read-optimized kernel fast path: concurrency-friendly lookups &#40;e.g., RCU-based indexing&#41; designed for high skew and low tail latency.)

[//]: # ()
[//]: # (- Clear correctness boundary: user space remains the source of truth; kernel cache is an acceleration layer with version-based invalidation and configurable staleness/TTL handling.)

[//]: # ()
[//]: # (- Operational safety by design: hard memory caps, pressure-aware reclamation, fail-open/fallback behavior, and rich telemetry/events for control-loop tuning.)

[//]: # ()
[//]: # (- Multi-tenant readiness: policy-driven budgeting and isolation to reduce cross-tenant interference under shared-cache contention.)

[//]: # ()
[//]: # (- Hotspot scalability options: support for per-core replica policies to mitigate cacheline contention on ultra-hot keys.)

[//]: # ()
[//]: # (### Limitations)

[//]: # ()
[//]: # (- Not a fully transparent “drop-in Redis in-kernel”: HybridKV intentionally does not implement full Redis semantics in kernel; it accelerates only a well-defined hot-read subset.)

[//]: # ()
[//]: # (- Consistency is policy-defined: strict invalidation is supported, but bounded staleness modes require careful configuration and workload validation.)

[//]: # ()
[//]: # (- Kernel complexity and deployment constraints: kernel components demand stricter testing, observability, and operational discipline than pure user-space solutions.)

[//]: # ()
[//]: # (- Best performance requires hot-set stability: workloads with rapidly shifting hot keys or write-heavy patterns may see reduced benefit due to invalidation/promotion churn.)

[//]: # ()
[//]: # (## When to use HybridKV)

[//]: # ()
[//]: # (- HybridKV is a strong fit for:)

[//]: # ()
[//]: # (    - Read-heavy KV workloads with strong skew &#40;hot keys dominate traffic&#41;)

[//]: # ()
[//]: # (    - Low-latency services where p99/p99.9 matters &#40;request routing, feature flags, configs, session tokens, metadata lookups&#41;)

[//]: # ()
[//]: # (    - Co-located architectures &#40;application + KV engine on the same host&#41; where syscall-based fast paths are effective)

[//]: # ()
[//]: # (    - Multi-tenant environments needing explicit cache budgets and isolation policies)

[//]: # ()
[//]: # (- HybridKV is not the best fit for:)

[//]: # ()
[//]: # (    - Write-heavy workloads with frequent updates to hot keys)

[//]: # ()
[//]: # (    - Workloads requiring strict cross-key transactional semantics at the cache layer)

[//]: # ()
[//]: # (    - Environments where kernel deployment is restricted or where a pure user-space cache is operationally preferred)

[//]: # ()
## Architecture

### High-Level Overview

HybridKV is built as a Cargo workspace with six modules:

```
hybridkv/
├── hkv-engine/    # Core storage engine (String, List, Hash, Set, ZSet)
├── hkv-server/    # Network server with hot data tracking
├── hkv-client/    # Client library with smart routing
├── hkv-kernel/    # Kernel module (framework: data plane + policies)
├── hkv-common/    # Shared types and protocols
└── hkv-gui/       # Web-based management dashboard
```

[//]: # ()
[//]: # (### Framework Architecture: Two-Plane Design)

[//]: # ()
[//]: # (```)

[//]: # (┌───────────────────────────────────────────────────────────────────────────┐)

[//]: # (│                         HybridKV Framework                                 │)

[//]: # (│                                                                            │)

[//]: # (│  ┌────────────────────────────────────────────────────────────────────┐  │)

[//]: # (│  │                         KERNEL SPACE                                │  │)

[//]: # (│  │  ┌──────────────────────────────────────────────────────────────┐  │  │)

[//]: # (│  │  │              KERNEL DATA PLANE &#40;Mechanism&#41;                    │  │  │)

[//]: # (│  │  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐ │  │  │)

[//]: # (│  │  │  │  Object  │  │   RCU    │  │  Memory  │  │ Invalidation │ │  │  │)

[//]: # (│  │  │  │ Storage  │  │   Hash   │  │ Governor │  │  & Version   │ │  │  │)

[//]: # (│  │  │  │  &#40;Slab&#41;  │  │  Table   │  │ &#40;Limits&#41; │  │   Tracking   │ │  │  │)

[//]: # (│  │  │  └──────────┘  └──────────┘  └──────────┘  └──────────────┘ │  │  │)

[//]: # (│  │  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐ │  │  │)

[//]: # (│  │  │  │   Stats  │  │ Char Dev │  │ Netlink  │  │   Safety &   │ │  │  │)

[//]: # (│  │  │  │&#40;Counters&#41;│  │/dev/hkv  │  │ Events   │  │   Fallback   │ │  │  │)

[//]: # (│  │  │  └──────────┘  └──────────┘  └──────────┘  └──────────────┘ │  │  │)

[//]: # (│  │  └────────────────────────┬─────────────────────────────────────┘  │  │)

[//]: # (│  │                           │ Policy Interface &#40;Rust Traits&#41;         │  │)

[//]: # (│  │  ┌────────────────────────▼─────────────────────────────────────┐  │  │)

[//]: # (│  │  │              POLICY PLANE &#40;Pluggable Decisions&#41;              │  │  │)

[//]: # (│  │  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐ │  │  │)

[//]: # (│  │  │  │ Eviction │  │Admission │  │ Hotness  │  │   Tenant     │ │  │  │)

[//]: # (│  │  │  │ Policies │  │ Control  │  │Estimator │  │   Budget     │ │  │  │)

[//]: # (│  │  │  │          │  │          │  │          │  │  & Fairness  │ │  │  │)

[//]: # (│  │  │  │ • LRU    │  │•Threshold│  │• CMS     │  │              │ │  │  │)

[//]: # (│  │  │  │ • LFU    │  │• TinyLFU │  │• Sampling│  │• Hard Quota  │ │  │  │)

[//]: # (│  │  │  │ • SLRU   │  │• Size-   │  │• Tiered  │  │• Proportional│ │  │  │)

[//]: # (│  │  │  │ • TwoQ   │  │  Aware   │  │ Counters │  │• Priority    │ │  │  │)

[//]: # (│  │  │  │ • FIFO   │  │          │  │          │  │  Based       │ │  │  │)

[//]: # (│  │  │  └──────────┘  └──────────┘  └──────────┘  └──────────────┘ │  │  │)

[//]: # (│  │  │  ┌──────────────────────────────────────────────────────────┐ │  │  │)

[//]: # (│  │  │  │         TTL & Consistency Policies                       │ │  │  │)

[//]: # (│  │  │  │  • Strict Invalidation  • Bounded Staleness              │ │  │  │)

[//]: # (│  │  │  │  • Version-Based Check  • Async Refresh                  │ │  │  │)

[//]: # (│  │  │  └──────────────────────────────────────────────────────────┘ │  │  │)

[//]: # (│  │  └──────────────────────────────────────────────────────────────┘  │  │)

[//]: # (│  └────────────────────────────────────────────────────────────────────┘  │)

[//]: # (│                                    ▲                                      │)

[//]: # (│                                    │ ioctl &#40;sync&#41; / Netlink &#40;async&#41;       │)

[//]: # (│                                    │                                      │)

[//]: # (│  ┌────────────────────────────────┴───────────────────────────────────┐  │)

[//]: # (│  │                          USER SPACE                                 │  │)

[//]: # (│  │  ┌──────────────────────────────────────────────────────────────┐  │  │)

[//]: # (│  │  │  hkv-server &#40;Control Plane&#41;                                   │  │  │)

[//]: # (│  │  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐ │  │  │)

[//]: # (│  │  │  │   RESP   │  │   Hot    │  │Promotion │  │    Admin     │ │  │  │)

[//]: # (│  │  │  │ Protocol │  │ Tracking │  │ Manager  │  │     API      │ │  │  │)

[//]: # (│  │  │  │  Parser  │  │  &#40;CMS&#41;   │  │&#40;Bg Thread&#41;│  │ &#40;REST/WS&#41;   │ │  │  │)

[//]: # (│  │  │  └──────────┘  └──────────┘  └──────────┘  └──────────────┘ │  │  │)

[//]: # (│  │  └──────────────────────────────────────────────────────────────┘  │  │)

[//]: # (│  │  ┌──────────────────────────────────────────────────────────────┐  │  │)

[//]: # (│  │  │  hkv-engine &#40;Storage Engine&#41;                                  │  │  │)

[//]: # (│  │  │  • In-memory data structures &#40;String/List/Hash/Set/ZSet&#41;      │  │  │)

[//]: # (│  │  │  • Persistence &#40;AOF/RDB&#41;                                       │  │  │)

[//]: # (│  │  │  • TTL management                                              │  │  │)

[//]: # (│  │  └──────────────────────────────────────────────────────────────┘  │  │)

[//]: # (│  └────────────────────────────────────────────────────────────────────┘  │)

[//]: # (│                                    ▲                                      │)

[//]: # (│                                    │ Client Library &#40;hkv-client&#41;          │)

[//]: # (│                                    │                                      │)

[//]: # (│  ┌────────────────────────────────┴───────────────────────────────────┐  │)

[//]: # (│  │                      Client Application                             │  │)

[//]: # (│  │  ┌──────────────┐      Fast Path: Kernel cache hit &#40;2-10μs&#41;        │  │)

[//]: # (│  │  │  Try Kernel  ├─────► Slow Path: User-space miss &#40;20-40μs&#41;       │  │)

[//]: # (│  │  │   First?     │      Fallback: On kernel error                    │  │)

[//]: # (│  │  └──────────────┘                                                    │  │)

[//]: # (│  └────────────────────────────────────────────────────────────────────┘  │)

[//]: # (└───────────────────────────────────────────────────────────────────────────┘)

[//]: # (```)

[//]: # ()
[//]: # (### Request Flow)

[//]: # ()
[//]: # (**Read Path &#40;Fast Path&#41;**:)

[//]: # (```)

[//]: # (1. Client → hkv-client.get&#40;"key"&#41;)

[//]: # (2. hkv-client → ioctl&#40;CMD_READ, "key"&#41; to /dev/hybridkv)

[//]: # (3. Kernel → RCU hash lookup &#40;lock-free, 20-50ns&#41;)

[//]: # (4. If HIT:)

[//]: # (   → copy_to_user &#40;value&#41;)

[//]: # (   → Policy: update LRU/access counters)

[//]: # (   → Return to client &#40;total: 2-10μs&#41;)

[//]: # (5. If MISS:)

[//]: # (   → Fallback to user-space)

[//]: # (   → hkv-client → TCP/RESP to hkv-server)

[//]: # (   → hkv-server → hkv-engine.get&#40;"key"&#41;)

[//]: # (   → Return to client &#40;total: 20-40μs&#41;)

[//]: # (```)

[//]: # ()
[//]: # (**Write Path &#40;Write-Through&#41;**:)

[//]: # (```)

[//]: # (1. Client → hkv-client.set&#40;"key", "value"&#41;)

[//]: # (2. hkv-client → TCP/RESP to hkv-server &#40;always user-space&#41;)

[//]: # (3. hkv-server → hkv-engine.set&#40;"key", "value"&#41;)

[//]: # (4. hkv-server → ioctl&#40;CMD_INVALIDATE, "key"&#41; to kernel)

[//]: # (5. Kernel → Mark entry stale or remove &#40;consistency mode&#41;)

[//]: # (6. Background: Promotion Manager decides if re-promotion needed)

[//]: # (```)

[//]: # ()
[//]: # (**Promotion Flow &#40;Background&#41;**:)

[//]: # (```)

[//]: # (1. Hot Tracker → Count-Min Sketch tracks all accesses)

[//]: # (2. Promotion Manager &#40;5-sec interval&#41;:)

[//]: # (   → Analyze hot set &#40;frequency > 100 QPS, read ratio > 90%&#41;)

[//]: # (   → Policy: Admission control decides what to promote)

[//]: # (   → ioctl&#40;CMD_BATCH_PROMOTE, keys[]&#41; to kernel)

[//]: # (3. Kernel → Policy: Eviction if memory pressure)

[//]: # (   → Insert into RCU hash table)

[//]: # (   → Netlink notify user-space of evictions)

[//]: # (```)

[//]: # ()
[//]: # (**Eviction Flow &#40;Autonomous&#41;**:)

[//]: # (```)

[//]: # (1. Kernel: Memory usage > 80% watermark)

[//]: # (2. Policy: EvictionPolicy.select_victims&#40;count=100&#41;)

[//]: # (   → LRU: Pick least recently used)

[//]: # (   → LFU: Pick least frequently used)

[//]: # (   → SLRU: Evict from probation list first)

[//]: # (3. Kernel: Remove entries, free memory)

[//]: # (4. Kernel → Netlink: Notify user-space of evicted keys)

[//]: # (5. User-space: Update hot tracking state)

[//]: # (```)

[//]: # ()
[//]: # (### Multi-Tenant Isolation)

[//]: # ()
[//]: # (```)

[//]: # (┌────────────────────────────────────────────────────────┐)

[//]: # (│              Kernel Cache &#40;256MB Total&#41;                 │)

[//]: # (├────────────────────────────────────────────────────────┤)

[//]: # (│  Tenant A &#40;100MB quota&#41;    │  Tenant B &#40;100MB quota&#41;   │)

[//]: # (│  ┌──────────────────────┐  │  ┌──────────────────────┐│)

[//]: # (│  │ 80MB used            │  │  │ 60MB used            ││)

[//]: # (│  │ 40K entries          │  │  │ 30K entries          ││)

[//]: # (│  │ Hit rate: 85%        │  │  │ Hit rate: 75%        ││)

[//]: # (│  └──────────────────────┘  │  └──────────────────────┘│)

[//]: # (│                            │                           │)

[//]: # (│  Shared Pool &#40;56MB available for proportional sharing&#41; │)

[//]: # (│  ┌───────────────────────────────────────────────────┐ │)

[//]: # (│  │ Allocated based on weight & min guarantees        │ │)

[//]: # (│  └───────────────────────────────────────────────────┘ │)

[//]: # (└────────────────────────────────────────────────────────┘)

[//]: # ()
[//]: # (Isolation Features:)

[//]: # (• Hard quotas prevent one tenant from starving others)

[//]: # (• Independent eviction domains &#40;tenant A eviction ≠ tenant B&#41;)

[//]: # (• Per-tenant statistics and monitoring)

[//]: # (• Priority-based allocation &#40;production > batch workloads&#41;)

[//]: # (```)

[//]: # ()
[//]: # (**How It Works**:)

[//]: # (1. All writes go to user-space storage engine &#40;hkv-engine&#41; - authoritative)

[//]: # (2. Background thread tracks hot keys and promotes to kernel cache)

[//]: # (3. Reads check kernel cache first &#40;fast path&#41;, fall back to user space)

[//]: # (4. Kernel autonomously evicts cold keys using configured policy &#40;LRU/LFU/etc.&#41;)

[//]: # (5. Multi-tenant policies ensure fairness and isolation under contention)

[//]: # ()
[//]: # ()
[//]: # (## Documentation)

[//]: # ()
[//]: # (Still working...)

[//]: # ()
[//]: # ()
[//]: # ()
