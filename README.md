# ApexBaseMEV

Ultra-low latency MEV extraction and Bug Bounty detection engine for Base L2.

## Architecture

### Core Components

- **Provider** (`src/provider.rs`): IPC-based connection to Reth node with `tokio::select!` concurrent block and pending transaction monitoring. Optimized for AWS us-east-1 latency.

- **Pathfinder** (`src/pathfinder.rs`): Graph-based arbitrage detection using SPFA (Shortest Path Faster Algorithm) for negative cycle detection. Implements Newton-Raphson method for optimal input amount calculation.

- **Simulator** (`src/sim.rs`): State-aware shadow execution using `revm`. Includes honeypot detection through immediate sell-after-buy simulation.

- **Engine** (`src/engine.rs`): Event loop orchestrating mempool monitoring, opportunity detection, and execution pipeline.

## Dependencies

- **alloy**: Ethereum interaction with IPC/WebSocket support
- **tokio**: Async runtime with multi-threading
- **revm**: EVM simulation engine
- **crossbeam**: Lock-free channels for inter-thread communication
- **fixed**: High-precision fixed-point arithmetic
- **dashmap**: Concurrent hash maps for price data

## Configuration

Edit `EngineConfig` in `src/main.rs`:

```rust
EngineConfig {
    ipc_path: "/tmp/reth.ipc".to_string(),
    region: "us-east-1",
    max_path_length: 4,
    min_profit_basis_points: 50,
}
```

## Build

```bash
cargo build --release
```

## Run

```bash
cargo run --release
```

## Technical Standards

- Rust 1.80+
- Zero-copy deserialization for blockchain logs
- Lock-free data structures via `Arc<RwLock<T>>` and crossbeam channels
- No emojis, professional documentation only
