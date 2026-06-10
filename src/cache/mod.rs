//! 🔄 Pool State Cache Module
//! 
//! Cache thread-safe de estados de pools com atualização via WebSocket events.
//! Nunca faz chamadas RPC durante o loop de detecção.

pub mod pool_cache;
pub mod multicall_bootstrap;
pub mod simple_bootstrap;

pub use pool_cache::{
    PoolCache, PoolState, CacheStats, Multicall3Call,
    build_getreserves_multicall, decode_getreserves_result,
    MULTICALL3_ADDRESS,
};

pub use multicall_bootstrap::{
    MulticallBootstrap, BootstrapConfig,
};

pub use simple_bootstrap::{
    bootstrap_simple,
};
