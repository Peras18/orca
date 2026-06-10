//! 🕸️ Arbitrage Graph Module
//!
//! Grafo de tokens para deteção de ciclos de arbitragem.
//! Suporta ciclos 2-hop e 3-hop com ordenação por lucro líquido.

pub mod arb_graph;
pub mod pool_scorer;
pub use arb_graph::{ArbGraph, ArbPath, Edge, GraphStats};
pub use pool_scorer::PoolScorer;
pub mod persistent_topology;
pub use persistent_topology::{PersistentTopology, BirthSignal};
