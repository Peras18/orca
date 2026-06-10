//! Pool Discovery - Auto-Scan de Factories Uniswap V3 & Aerodrome
//!
//! Sistema de descoberta contínua que:
//! - Varre factories via event logs (economia de créditos Alchemy)
//! - Filtra pools por TVL > $10k e Volume 24h > $5k
//! - Prioriza tokens base (WETH, USDC, DAI, CBETH)
//! - Atualiza o radar a cada 5 minutos

pub mod pool_discovery;

pub use pool_discovery::{
    PoolDiscoveryEngine,
    PoolData,
    DiscoveryConfig,
    DiscoveryStats,
    DexType,
    UNISWAP_V3_FACTORY,
    AERODROME_FACTORY,
    UNISWAP_V2_FACTORY,
    WETH, USDC, DAI, CBETH,
};
