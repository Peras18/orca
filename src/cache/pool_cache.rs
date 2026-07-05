//! 🔄 Pool State Cache - In-Memory with WebSocket Sync Updates
//!
//! Design principles:
//! - Initialize ALL reserves via multicall (1 RPC call, not N)
//! - Update ONLY via Sync events (zero RPC calls during normal operation)
//! - Never make RPC calls during opportunity calculation
//! - Support V2, Stable (sAMM Aerodrome), and V3 pools

use alloy::primitives::{Address, U256};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::contracts::DexType;

/// Endereço Multicall3 na BASE
pub const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";

/// Estado completo de um pool
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolState {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub fee: u32,
    pub dex_type: DexType,
    /// Bloco da última atualização
    pub last_update_block: u64,
    /// Bloco da última atualização REAL de reservas/liquidez (diferente de
    /// last_update_block, que também é "tocado" por eventos sem dados novos
    /// -- usado por is_stale() para detectar pools genuinamente congeladas,
    /// mesmo que continuem a receber touch() de swaps não-relacionados).
    pub last_real_sync_block: u64,
    /// Timestamp da última atualização (unix millis)
    pub last_update_time: u64,
    /// Decimals do token0
    pub decimals0: u8,
    /// Decimals do token1
    pub decimals1: u8,
    /// TVL aproximado em ETH (para filtragem)
    pub tvl_eth: U256,
    /// sqrt(price) * 2^96 — apenas para pools V3/Slipstream
    pub sqrt_price_x96: Option<u128>,
    /// Liquidez ativa no tick atual — apenas para pools V3/Slipstream
    pub liquidity: Option<u128>,
    /// Tick atual — apenas para pools V3/Slipstream
    pub tick: Option<i32>,
    /// Reserves verificadas via getReserves() real (não sintéticas)
    pub reserve_verified: bool,
}

impl PoolState {
    /// Cria novo pool state com valores zerados
    pub fn new(
        address: Address,
        token0: Address,
        token1: Address,
        fee: u32,
        dex_type: DexType,
    ) -> Self {
        Self {
            address,
            token0,
            token1,
            reserve0: U256::ZERO,
            reserve1: U256::ZERO,
            fee,
            dex_type,
            last_update_block: 0,
            last_real_sync_block: 0,
            last_update_time: 0,
            decimals0: 18,
            decimals1: 18,
            tvl_eth: U256::ZERO,
            sqrt_price_x96: None,
            liquidity: None,
            tick: None,
            reserve_verified: false,
        }
    }

    /// Verifica se as reserves são válidas (não zero)
    pub fn has_liquidity(&self) -> bool {
        // CORREÇÃO: para pools V3, reserve0/reserve1 são uma aproximação
        // sintética sempre preenchida com algo não-zero -- isso deixava
        // passar pools V3 com liquidity() REAL = 0 (criadas mas nunca
        // tiveram depósito, ou liquidez totalmente removida). Confirmado
        // on-chain: pool 0xfd51554381c7a03b3a6ed5e28c216b1aa2b51c8c existe,
        // mas liquidity()=0 -- causava "Unexpected error" no QuoterV2 e
        // "IIA" no nosso contrato, sempre, independentemente do tamanho do
        // swap testado (mesmo 4 wei revertia). Para V3/Slipstream, exigir
        // também liquidity > 0 explicitamente.
        let has_reserves = !self.reserve0.is_zero() && !self.reserve1.is_zero();
        if matches!(self.dex_type, crate::contracts::DexType::UniswapV3 | crate::contracts::DexType::PancakeSwap) {
            has_reserves && self.liquidity.unwrap_or(0) > 0
        } else {
            has_reserves
        }
    }

    /// Verifica se os dados estão stale (sem actualizacão recente).
    ///
    /// Limiar de 500 blocos (~250 segundos na Base a 0.5s/bloco).
    ///
    /// Razão para valor alto:
    ///  - Pools Aerodrome vAMM de alto volume (WETH/USDC) emitem Sync events
    ///    frequentemente, mas dependem de swaps chegarem ao nosso filtro.
    ///  - Pools bootstrapadas com reserves reais são válidas até receberem eventos.
    ///  - Valor baixo (100 blocos = 50s) causava expulsao prematura de pools
    ///    que não recebiam eventos nessa janela curta.
    ///  - 500 blocos = ~4 minutos: qualquer pool activa certamente recebe
    ///    pelo menos um evento nesse período.
    pub fn is_stale(&self, current_block: u64) -> bool {
        // CORREÇÃO: usar last_real_sync_block, não last_update_block --
        // touch() actualiza last_update_block sem nunca mudar reservas
        // reais, fazendo pools genuinamente congeladas (confirmado: zero
        // eventos Sync reais em 2.7h+) nunca serem marcadas como stale.
        current_block.saturating_sub(self.last_real_sync_block) > 500
    }

    /// Calcula preço spot (token1/token0)
    pub fn spot_price(&self) -> Option<U256> {
        if self.reserve0.is_zero() {
            return None;
        }
        // Normalizar por decimals
        let reserve0_norm = self.normalize_amount(self.reserve0, self.decimals0);
        let reserve1_norm = self.normalize_amount(self.reserve1, self.decimals1);
        Some((reserve1_norm * U256::from(10).pow(U256::from(18))) / reserve0_norm)
    }

    /// Normaliza amount para 18 decimals
    fn normalize_amount(&self, amount: U256, decimals: u8) -> U256 {
        if decimals == 18 {
            amount
        } else if decimals < 18 {
            amount * U256::from(10).pow(U256::from(18 - decimals))
        } else {
            amount / U256::from(10).pow(U256::from(decimals - 18))
        }
    }

    /// Atualiza reserves via evento Sync (V2/Stable)
    pub fn update_reserves_v2(&mut self, reserve0: U256, reserve1: U256, block: u64) {
        self.reserve0 = reserve0;
        self.reserve1 = reserve1;
        self.last_update_block = block;
        self.last_update_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Estimar TVL
        self.estimate_tvl();
    }

    /// Atualiza estado via evento Swap (V3)
    pub fn update_reserves_v3(&mut self, sqrt_price_x96: U256, liquidity: u128, block: u64) {
        // Para V3, calculamos reserves virtuais a partir de sqrtPriceX96 e liquidez
        //   reserve0 = L * 2^96 / sqrtPriceX96
        //   reserve1 = L * sqrtPriceX96 / 2^96
        let q96 = U256::from(1u128) << 96;
        let liq = U256::from(liquidity);

        self.reserve0 = liq
            .checked_mul(q96)
            .and_then(|v| v.checked_div(sqrt_price_x96))
            .unwrap_or(U256::ZERO);
        self.reserve1 = liq
            .checked_mul(sqrt_price_x96)
            .and_then(|v| v.checked_div(q96))
            .unwrap_or(U256::ZERO);

        self.sqrt_price_x96 = Some(sqrt_price_x96.try_into().unwrap_or(u128::MAX));
        self.liquidity = Some(liquidity);
        self.last_real_sync_block = block;

        self.last_update_block = block;
        self.last_update_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.estimate_tvl();
    }

    /// Estima TVL em ETH (simplificado)
    fn estimate_tvl(&mut self) {
        // Normaliza reserve0 para 18 decimais antes de estimar TVL
        let reserve0_norm = if self.decimals0 == 18 {
            self.reserve0
        } else if self.decimals0 < 18 {
            self.reserve0
                .saturating_mul(U256::from(10u64).pow(U256::from((18 - self.decimals0) as u64)))
        } else {
            self.reserve0 / U256::from(10u64).pow(U256::from((self.decimals0 - 18) as u64))
        };
        self.tvl_eth = reserve0_norm.saturating_mul(U256::from(2));
    }
}

/// Cache thread-safe de pools
#[derive(Clone, Debug)]
pub struct PoolCache {
    /// Mapa de endereço -> estado do pool
    pools: Arc<DashMap<Address, PoolState>>,
    /// Set de pools ativos (TVL > 10 ETH)
    active_pools: Arc<RwLock<Vec<Address>>>,
    /// Contador de updates
    update_count: Arc<RwLock<u64>>,
    /// Bloco da última limpeza
    last_cleanup_block: Arc<RwLock<u64>>,
}

impl PoolCache {
    /// Cria novo cache vazio
    pub fn new() -> Self {
        Self {
            pools: Arc::new(DashMap::new()),
            active_pools: Arc::new(RwLock::new(Vec::new())),
            update_count: Arc::new(RwLock::new(0)),
            last_cleanup_block: Arc::new(RwLock::new(0)),
        }
    }

    /// Insere ou atualiza um pool
    pub fn insert(&self, state: PoolState) {
        self.pools.insert(state.address, state);
    }

    /// Obtém estado de um pool
    pub fn get(&self, address: &Address) -> Option<PoolState> {
        self.pools.get(address).map(|entry| entry.clone())
    }

    /// Atualiza token0/token1 de um pool (essencial para construir edges)
    pub fn update_tokens(&self, addr: Address, token0: Address, token1: Address) {
        if let Some(mut entry) = self.pools.get_mut(&addr) {
            entry.token0 = token0;
            entry.token1 = token1;
        }
    }

    /// Atualiza reserves via evento Sync (chamado pelo WebSocket listener)
    pub fn update_sync_event(
        &self,
        pool_address: Address,
        reserve0: U256,
        reserve1: U256,
        block: u64,
    ) {
        if let Some(mut entry) = self.pools.get_mut(&pool_address) {
            entry.update_reserves_v2(reserve0, reserve1, block);

            let mut count = self.update_count.write();
            *count += 1;

            debug!(
                "🔄 [PoolCache] Sync update | Pool: {:?} | Block: {} | Updates: {}",
                pool_address, block, *count
            );
        }
    }

    /// Atualiza via evento Swap V3
    pub fn update_swap_event(
        &self,
        pool_address: Address,
        sqrt_price_x96: U256,
        liquidity: u128,
        block: u64,
    ) {
        if let Some(mut entry) = self.pools.get_mut(&pool_address) {
            entry.update_reserves_v3(sqrt_price_x96, liquidity, block);

            let mut count = self.update_count.write();
            *count += 1;
        }
    }

    /// Atualiza apenas o bloco de última atualização — não altera reserves.
    /// Usar para marcar um pool como "visto" sem destruir reserves reais.
    pub fn touch(&self, pool_address: Address, block: u64) {
        if let Some(mut entry) = self.pools.get_mut(&pool_address) {
            entry.last_update_block = block;
        }
    }

    /// Retorna todos os pools ativos (TVL >= min_tvl_eth)
    pub fn get_active_pools(&self, min_tvl_eth: U256) -> Vec<PoolState> {
        self.pools
            .iter()
            .filter(|entry| entry.value().tvl_eth >= min_tvl_eth)
            .map(|entry| entry.clone())
            .collect()
    }

    /// Retorna pools por par de tokens
    pub fn get_pools_by_tokens(&self, token_a: Address, token_b: Address) -> Vec<PoolState> {
        self.pools
            .iter()
            .filter(|entry| {
                let state = entry.value();
                (state.token0 == token_a && state.token1 == token_b)
                    || (state.token0 == token_b && state.token1 == token_a)
            })
            .map(|entry| entry.clone())
            .collect()
    }

    /// Retorna vizinhos de um token (pools que contêm este token)
    pub fn get_token_neighbors(&self, token: Address) -> Vec<(Address, PoolState)> {
        self.pools
            .iter()
            .filter(|entry| entry.value().token0 == token || entry.value().token1 == token)
            .map(|entry| (*entry.key(), entry.clone()))
            .collect()
    }

    /// Verifica se pool existe no cache
    pub fn contains(&self, address: &Address) -> bool {
        self.pools.contains_key(address)
    }

    /// Número total de pools no cache
    pub fn len(&self) -> usize {
        self.pools.len()
    }

    /// Limpa pools inativos (sem updates por > 100 blocos)
    pub fn cleanup_stale_pools(&self, current_block: u64) {
        let mut last_cleanup = self.last_cleanup_block.write();

        // Só limpa a cada 50 blocos
        if current_block.saturating_sub(*last_cleanup) < 50 {
            return;
        }

        let stale_threshold = 100;
        let to_remove: Vec<Address> = self
            .pools
            .iter()
            .filter(|entry| {
                current_block.saturating_sub(entry.value().last_update_block) > stale_threshold
            })
            .map(|entry| *entry.key())
            .collect();

        for addr in &to_remove {
            self.pools.remove(addr);
        }

        if !to_remove.is_empty() {
            info!(
                "🧹 [PoolCache] Removed {} stale pools | Remaining: {}",
                to_remove.len(),
                self.pools.len()
            );
        }

        *last_cleanup = current_block;
    }

    /// Estatísticas do cache
    pub fn stats(&self) -> CacheStats {
        let total = self.pools.len();
        let active = self
            .pools
            .iter()
            .filter(|e| e.value().has_liquidity())
            .count();
        let updates = *self.update_count.read();

        CacheStats {
            total_pools: total,
            active_pools: active,
            total_updates: updates,
        }
    }

    /// 🔬 Conta pools com reserves válidas (> 0)
    pub fn count_pools_with_reserves(&self) -> usize {
        self.pools
            .iter()
            .filter(|e| !e.value().reserve0.is_zero() && !e.value().reserve1.is_zero())
            .count()
    }

    /// 🔬 Retorna os primeiros N pools para diagnóstico
    pub fn get_sample_pools(&self, n: usize) -> Vec<PoolState> {
        self.pools.iter().take(n).map(|e| e.clone()).collect()
    }

    /// Inicializa cache a partir de arquivo JSON
    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        let pools: Vec<PoolState> = serde_json::from_str(data)?;
        let cache = Self::new();

        for state in pools {
            cache.insert(state);
        }

        info!("📦 [PoolCache] Loaded {} pools from JSON", cache.len());
        Ok(cache)
    }

    /// Exporta para JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let pools: Vec<PoolState> = self.pools.iter().map(|e| e.clone()).collect();
        serde_json::to_string_pretty(&pools)
    }
}

impl Default for PoolCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Estatísticas do cache
#[derive(Clone, Debug)]
pub struct CacheStats {
    pub total_pools: usize,
    pub active_pools: usize,
    pub total_updates: u64,
}

/// Multicall3 aggregate3 call data
#[derive(Clone, Debug)]
pub struct Multicall3Call {
    pub target: Address,
    pub allow_failure: bool,
    pub call_data: Vec<u8>,
}

/// Helper para construir multicall de getReserves
pub fn build_getreserves_multicall(pools: &[Address]) -> Vec<Multicall3Call> {
    // Selector getReserves() = 0x0902f1ac
    let selector = vec![0x09, 0x02, 0xf1, 0xac];

    pools
        .iter()
        .map(|&pool| Multicall3Call {
            target: pool,
            allow_failure: true,
            call_data: selector.clone(),
        })
        .collect()
}

/// Decodifica resultado de getReserves
/// Retorna (reserve0, reserve1, blockTimestampLast)
pub fn decode_getreserves_result(data: &[u8]) -> Option<(U256, U256, u32)> {
    if data.len() < 96 {
        return None;
    }

    // getReserves retorna (uint112, uint112, uint32) = 32 + 32 + 32 bytes (padded)
    let reserve0 = U256::from_be_slice(&data[0..32]);
    let reserve1 = U256::from_be_slice(&data[32..64]);
    let block_timestamp = u32::from_be_bytes([data[92], data[93], data[94], data[95]]);

    Some((reserve0, reserve1, block_timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_pool_state_creation() {
        let addr = address!("0x4200000000000000000000000000000000000006");
        let token0 = address!("0x4200000000000000000000000000000000000006");
        let token1 = address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

        let state = PoolState::new(addr, token0, token1, 3000, DexType::UniswapV2);

        assert!(!state.has_liquidity());
        assert_eq!(state.fee, 3000);
    }

    #[test]
    fn test_cache_operations() {
        let cache = PoolCache::new();
        let addr = address!("0x4200000000000000000000000000000000000006");
        let token0 = address!("0x4200000000000000000000000000000000000006");
        let token1 = address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

        let state = PoolState::new(addr, token0, token1, 3000, DexType::UniswapV2);
        cache.insert(state.clone());

        assert!(cache.contains(&addr));
        assert_eq!(cache.len(), 1);

        // Test update
        cache.update_sync_event(addr, U256::from(1000000), U256::from(2000000), 100);

        let updated = cache.get(&addr).unwrap();
        assert_eq!(updated.reserve0, U256::from(1000000));
        assert_eq!(updated.last_update_block, 100);
    }
}
