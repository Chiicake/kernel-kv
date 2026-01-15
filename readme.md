# HybridKV
A high-performance hybrid KV storage system optimized for Linux, designed to solve the user-kernel mode switch/copy overhead of traditional user-space KV systems (e.g., Redis) while ensuring system stability.
## Core Design
Cache hot keys (small-sized ≤1KB, read-heavy ≥90%, hit rate ≥20%) in a lightweight kernel-space hash table (hlist_bl_hash) to eliminate mode switches, reducing hot key query latency to 1-5μs (30-80% lower than pure user-space) and boosting read throughput to 1M+ QPS. Regular keys are processed in user space to retain full functionality (expiration, writes, large data) and avoid kernel resource exhaustion.
## Key Features
- Ultra-low latency/jitter for hot key reads (ideal for high-frequency trading/low-latency gateways)
- Kernel failover (auto-degrade to user-space on high lock contention/memory limits)
- Asynchronous batch sync for hot key updates (minimize overhead)
- Safe kernel primitives (rw_semaphore, LRU eviction) to prevent system panic
