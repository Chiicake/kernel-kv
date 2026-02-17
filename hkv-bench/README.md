# hkv-bench

A specialized benchmarking tool for HybridKV, designed to measure performance metrics (QPS, latency, throughput) and compare them against Redis.

## Design Goals
1. **Protocol Compatibility**: Use the RESP (Redis Serialization Protocol) to communicate with both `hkv-server` and `redis-server`.
2. **High Concurrency**: Leverage `tokio` to spawn thousands of lightweight tasks, simulating massive concurrent client connections.
3. **Pipelining Support**: Evaluate the server's ability to handle batched requests (pipeline mode).
4. **Detailed Metrics**: Report not just average QPS, but also P50, P99, P999 latency histograms to catch tail latency issues.

## Implementation Logic

### 1. Configuration (CLI Args)
The tool should accept command-line arguments similar to `redis-benchmark`:
- `-h <host>`: Server hostname (default: 127.0.0.1)
- `-p <port>`: Server port (default: 6379)
- `-c <clients>`: Number of parallel connections (default: 50)
- `-n <requests>`: Total number of requests (default: 100000)
- `-d <data-size>`: Data size of SET/GET values in bytes (default: 3)
- `-P <pipeline>`: Pipeline <numreq> requests (default: 1)
- `-t <tests>`: Only run the comma-separated list of tests (e.g. set,get)

### 2. Connection Management
- Use `tokio::net::TcpStream` to establish connections.
- Pre-allocate a pool of connections based on the `-c` flag to avoid connection setup overhead during the measurement phase.

### 3. Workload Generation
- **SET**: Generate random keys (e.g., `key:000001`) and values of specified size.
- **GET**: Query random keys from the range populated by the SET phase.
- **Pipeline**: If `-P` > 1, batch multiple commands into a single TCP write and read all responses in one go.

### 4. Execution Loop
For each client task:
1. Construct the RESP command buffer.
2. Record `start_time = Instant::now()`.
3. Write to the TCP stream.
4. Read the response from the TCP stream.
5. Record `latency = start_time.elapsed()`.
6. Send the latency sample to a central aggregator (via `mpsc` channel or atomic histogram).

### 5. Reporting
- Use a histogram library (like `hdrhistogram`) to aggregate latency data.
- Output a summary table to stdout:
  ```text
  ====== SET ======
    100000 requests completed in 0.89 seconds
    50 parallel clients
    3 bytes payload
    keep alive: 1

  99.91% <= 1 milliseconds
  112359.55 requests per second
  ```

## Usage
```bash
cargo run --release -p hkv-bench -- -t set,get -n 100000 -c 50
```
