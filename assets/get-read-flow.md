# HybridKV GET Read Flow

```mermaid
flowchart TD
    A[Client sends RESP GET] --> B[hkv-server main accept loop]
    B --> C[handle_connection]
    C --> D[RespParser.parse]
    D --> E[dispatch_command]
    E --> F[handle_get]
    F --> G[MemoryEngine.get]

    G --> H[shard_for key]
    H --> I[lookup key index in shard map]
    I --> J{found?}
    J -- no --> K[return RESP Null]

    J -- yes --> L[check TTL is_expired]
    L --> M{expired?}
    M -- yes --> N[remove_idx and update used_bytes]
    N --> K

    M -- no --> O[clone value and touch LRU]
    O --> P[return RESP Bulk value]

    K --> Q[write response to TCP stream]
    P --> Q
```

## Code anchors

- Connection entry: `hkv-server/src/main.rs:22`
- Request loop and parser: `hkv-server/src/server.rs:18`, `hkv-server/src/protocol.rs:53`
- Command dispatch and GET handler: `hkv-server/src/server.rs:47`, `hkv-server/src/server.rs:86`
- Engine read path: `hkv-engine/src/memory.rs:451`

## GitNexus evidence used

- `context` for `main`, `dispatch_command`, `handle_get`, `get`
- Process chain including `Main -> Resp_null` and `Get -> Shard_index`
