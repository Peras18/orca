//! 🦎 Long-tail MEV — Oportunidades ignoradas pelos grandes bots
//!
//! - Token launch sniping (legítimo)
//! - Cross-DEX divergence em mid-caps
//! - Stale oracle arbitrage

use alloy::primitives::{Address, U256};
use std::collections::HashSet;
use parking_lot::RwLock;
use tracing::info;

use crate::cache::pool_cache::PoolCache;

/// Scanner de tokens mid-cap (1M-50M market cap)
#[derive(Debug)]
pub struct MidCapScanner {
    pool_cache: PoolCache,
    /// Tokens mid-cap monitorizados
    tracked_tokens: RwLock<HashSet<Address>>,
    /// Mínimo TVL para considerar (50k USD)
    min_tvl_wei: U256,
}

impl MidCapScanner {
    pub fn new(pool_cache: PoolCache) -> Self {
        Self {
            pool_cache,
            tracked_tokens: RwLock::new(HashSet::new()),
            min_tvl_wei: U256::from(50_000) * U256::from(10).pow(U256::from(18)), // 50k ETH proxy
        }
    }

    /// Adiciona token à lista de monitorização
    pub fn track_token(&self, token: Address) {
        self.tracked_tokens.write().insert(token);
        info!("🦎 Tracking mid-cap token: {:?}", token);
    }

    /// Verifica divergências entre DEXs para tokens mid-cap
    pub fn find_divergences(&self) -> Vec<PriceDivergence> {
        let mut divergences = Vec::new();
        let tracked = self.tracked_tokens.read();

        for &token in tracked.iter() {
            // Obter todos os pools deste token
            let pools = self.get_pools_for_token(token);
            
            if pools.len() < 2 {
                continue;
            }

            // Calcular preços
            let mut prices: Vec<(Address, U256)> = Vec::new();
            for pool in &pools {
                if let Some(price) = pool.spot_price() {
                    prices.push((pool.address, price));
                }
            }

            // Encontrar maior divergência
            if prices.len() >= 2 {
                prices.sort_by(|a, b| b.1.cmp(&a.1));
                
                let highest = &prices[0];
                let lowest = &prices.last().unwrap();
                
                // Divergência > 0.5%
                let diff = highest.1 - lowest.1;
                let diff_bps = (diff * U256::from(10000)) / lowest.1;
                
                if diff_bps >= U256::from(50) { // 0.5%
                    divergences.push(PriceDivergence {
                        token,
                        high_pool: highest.0,
                        low_pool: lowest.0,
                        high_price: highest.1,
                        low_price: lowest.1,
                        spread_bps: diff_bps.to::<u32>(),
                    });
                }
            }
        }

        divergences
    }

    fn get_pools_for_token(&self, token: Address) -> Vec<crate::cache::pool_cache::PoolState> {
        // Usar cache para obter vizinhos
        self.pool_cache.get_token_neighbors(token)
            .into_iter()
            .map(|(_, state)| state)
            .filter(|p| p.has_liquidity())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct PriceDivergence {
    pub token: Address,
    pub high_pool: Address,
    pub low_pool: Address,
    pub high_price: U256,
    pub low_price: U256,
    pub spread_bps: u32,
}

/// Monitor de token launches
#[derive(Debug)]
pub struct LaunchMonitor {
    /// Pares criados nos últimos 3 blocos
    recent_pairs: RwLock<Vec<(Address, u64)>>, // (pool, block)
}

impl LaunchMonitor {
    pub fn new() -> Self {
        Self {
            recent_pairs: RwLock::new(Vec::new()),
        }
    }

    /// Registra novo par (chamado pelo pool_discovery)
    pub fn on_pair_created(&self, pool: Address, block: u64) {
        self.recent_pairs.write().push((pool, block));
    }

    /// Limpa pares antigos e retorna os atuais
    pub fn get_recent_launches(&self, current_block: u64) -> Vec<Address> {
        let mut pairs = self.recent_pairs.write();
        
        // Manter só os últimos 3 blocos
        pairs.retain(|(_, b)| current_block - *b <= 3);
        
        pairs.iter().map(|(p, _)| *p).collect()
    }
}
