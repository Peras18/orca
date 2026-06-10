//! 🏃 Backrunning — Reagir a swaps grandes imediatamente
//!
//! Detecta Swap events de valor > threshold, recalcula oportunidade
//! e submete no mesmo bloco ou seguinte.

use alloy::primitives::{Address, U256, U128};
use std::collections::HashMap;
use parking_lot::RwLock;
use tracing::{debug, info, warn};
use tokio::sync::mpsc;

use crate::cache::pool_cache::{PoolCache, PoolState};
use crate::graph::arb_graph::{ArbGraph, ArbPath};

/// Evento de swap grande detectado
#[derive(Clone, Debug)]
pub struct LargeSwapEvent {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
    pub block: u64,
    pub sender: Address,
}

/// Configuração do backrunner
pub struct BackrunConfig {
    /// Threshold: swap deve ser > 5% do TVL do pool
    pub tvl_threshold_bps: u32,
    /// Flash loan amounts a testar
    pub flash_loan_amounts: Vec<U256>,
    /// Mínimo lucro em wei
    pub min_profit_wei: U256,
    /// Canal de prioridade alta
    pub tx_sender: mpsc::Sender<ArbPath>,
}

/// Filtro de eventos Swap
pub struct SwapEventFilter {
    pool_cache: PoolCache,
    config: BackrunConfig,
    /// Thresholds por pool (5% do TVL)
    thresholds: RwLock<HashMap<Address, U256>>,
    /// Contador de eventos processados
    events_processed: RwLock<u64>,
}

impl std::fmt::Debug for SwapEventFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "SwapEventFilter") }
}
impl std::fmt::Debug for BackrunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "BackrunConfig") }
}
impl SwapEventFilter {
    pub fn new(pool_cache: PoolCache, config: BackrunConfig) -> Self {
        Self {
            pool_cache,
            config,
            thresholds: RwLock::new(HashMap::new()),
            events_processed: RwLock::new(0),
        }
    }

    /// Atualiza thresholds baseado em TVL atual
    pub fn update_thresholds(&self) {
        let pools = self.pool_cache.get_active_pools(U256::from(10).pow(U256::from(19))); // 10 ETH
        let mut thresholds = self.thresholds.write();
        
        for pool in pools {
            let threshold = pool.tvl_eth * U256::from(self.config.tvl_threshold_bps) / U256::from(10000);
            thresholds.insert(pool.address, threshold);
        }
    }

    /// Processa evento Swap recebido via WebSocket
    pub async fn on_swap_event(&self, event: LargeSwapEvent, current_block: u64) -> Option<ArbPath> {
        // STALENESS CHECK: rejeitar eventos de blocos antigos (> 1 bloco de atraso)
        let block_lag = current_block.saturating_sub(event.block);
        if block_lag > 1 {
            warn!(
                "🚫 Backrun STALE | Pool: {:?} | Event block: {} | Current: {} | Lag: {} blocks",
                event.pool, event.block, current_block, block_lag
            );
            return None;
        }

        // Verificar se é grande o suficiente
        let threshold = self.thresholds.read().get(&event.pool).copied()?;

        if event.amount_in < threshold {
            return None;
        }

        *self.events_processed.write() += 1;

        debug!(
            "🏃 Backrun candidate | Pool: {:?} | Amount: {:?} ETH | Block: {}",
            event.pool,
            event.amount_in / U256::from(10).pow(U256::from(18)),
            event.block
        );

        // Recalcular reserves pós-swap e atualizar cache
        if let Some(new_pool_state) = self.estimate_post_swap_reserves(&event) {
            self.pool_cache.insert(new_pool_state.clone());

            // CONSTRUIR ARBGRAPH e procurar oportunidades reais
            let mut graph = ArbGraph::new(self.pool_cache.clone(), U256::from(10).pow(U256::from(19)));
            graph.rebuild(current_block);

            // Procurar ciclos a partir do token de saída do swap grande
            let _weth = alloy::primitives::address!("0x4200000000000000000000000000000000000006");
            let start_token = if new_pool_state.token0 == event.token_in {
                new_pool_state.token1
            } else {
                new_pool_state.token0
            };

            let gas_price_wei = U256::from(1_000_000_000u64); // 1 gwei
            let opportunities = graph.find_opportunities(
                start_token,
                &self.config.flash_loan_amounts,
                gas_price_wei,
                1.0, // min_profit_ratio
            );

            // Retornar a melhor oportunidade
            if let Some(best) = opportunities.into_iter().next() {
                info!(
                    "🎯 Backrun OPPORTUNITY | Profit: {} wei | Hops: {} | Path: {}",
                    best.net_profit, best.hops.len(), best.unique_id()
                );
                return Some(best);
            }
        }

        None
    }

    /// Estima reserves após o swap grande
    fn estimate_post_swap_reserves(&self, event: &LargeSwapEvent) -> Option<PoolState> {
        let mut pool = self.pool_cache.get(&event.pool)?;
        
        // Simular o impacto do swap no pool
        // Para V2: as reserves mudam conforme a fórmula constant product
        let (reserve_in, reserve_out) = if pool.token0 == event.token_in {
            (pool.reserve0, pool.reserve1)
        } else {
            (pool.reserve1, pool.reserve0)
        };

        // Aproximação: o swap já aconteceu, então as novas reserves são:
        // new_reserve_in = reserve_in + amount_in
        // new_reserve_out = reserve_out - amount_out (com fee)
        let new_reserve_in = reserve_in + event.amount_in;
        let new_reserve_out = if reserve_out > event.amount_out {
            reserve_out - event.amount_out
        } else {
            U256::ZERO
        };

        if pool.token0 == event.token_in {
            pool.reserve0 = new_reserve_in;
            pool.reserve1 = new_reserve_out;
        } else {
            pool.reserve1 = new_reserve_in;
            pool.reserve0 = new_reserve_out;
        }

        Some(pool)
    }

    pub fn stats(&self) -> BackrunStats {
        BackrunStats {
            thresholds_defined: self.thresholds.read().len(),
            events_processed: *self.events_processed.read(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackrunStats {
    pub thresholds_defined: usize,
    pub events_processed: u64,
}

/// BatchBuilder para agrupar múltiplas oportunidades no mesmo bloco
pub struct BatchBuilder {
    pending: RwLock<Vec<ArbPath>>,
    wait_ms: u64,
}

impl BatchBuilder {
    pub fn new(wait_ms: u64) -> Self {
        Self {
            pending: RwLock::new(Vec::new()),
            wait_ms,
        }
    }

    pub fn add(&self, path: ArbPath) {
        self.pending.write().push(path);
    }

    /// Espera o tempo configurado e retorna batch
    pub async fn collect(&self) -> Vec<ArbPath> {
        tokio::time::sleep(tokio::time::Duration::from_millis(self.wait_ms)).await;
        let mut paths = self.pending.write();
        if paths.len() > 1 {
            // Retorna batch
            std::mem::take(&mut *paths)
        } else {
            // Single path - retorna imediatamente
            paths.drain(..).collect()
        }
    }
}
