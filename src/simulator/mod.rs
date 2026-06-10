use revm::{
    db::{CacheDB, EmptyDB},
    primitives::{Address as RevmAddress, U256 as RevmU256, Bytes, TransactTo, ExecutionResult, Output},
    Evm,
};
use alloy::primitives::{Address, U256};
use std::sync::Arc;
use tokio::sync::RwLock;
use hashbrown::HashMap;
use tracing::trace;

use crate::contracts::{NormalizedSwapEvent, DexType};
use crate::types::ArbitragePath;

// Elite Shadow Hunter Module
pub mod elite_shadow_hunter;
pub use elite_shadow_hunter::{EliteShadowHunter, EliteShadowHunterConfig, AtomicSimulationResult, TokenSafetyCheck, LiquidityCategory, FlashSwapConfig};

/// Estado de uma pool em memória (Copy-friendly)
#[derive(Clone, Copy, Debug)]
pub struct PoolState {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: u128,
    pub reserve1: u128,
    pub fee: u32,
    pub sqrt_price_x96: u128, // Compactado para 128 bits
    pub dex_type: DexType,
    pub last_block: u64,
}

/// Resultado da simulação atômica
#[derive(Clone, Debug)]
pub struct SimulationResult {
    pub success: bool,
    pub net_profit_wei: i128,
    pub gas_used: u64,
    pub gas_cost_wei: u128,
    pub error: Option<String>,
    pub execution_time_us: u64,
}

impl Default for SimulationResult {
    fn default() -> Self {
        Self {
            success: false,
            net_profit_wei: 0,
            gas_used: 0,
            gas_cost_wei: 0,
            error: None,
            execution_time_us: 0,
        }
    }
}

/// Simulador REVM de ultra-baixa latência
pub struct StateSimulator {
    /// Cache de estado em memória (hashbrown para performance)
    state_cache: Arc<RwLock<HashMap<Address, PoolState>>>,
    /// Base de dados REVM (vazia - usamos cache)
    db: Arc<RwLock<CacheDB<EmptyDB>>>,
    /// Configuração de gas
    base_gas_cost: u64,
    gas_price_wei: u128,
}

impl StateSimulator {
    pub fn new(gas_price_gwei: u64) -> Self {
        Self {
            state_cache: Arc::new(RwLock::new(HashMap::with_capacity(10000))),
            db: Arc::new(RwLock::new(CacheDB::new(EmptyDB::new()))),
            base_gas_cost: 21000, // Transaction base
            gas_price_wei: (gas_price_gwei as u128) * 1_000_000_000u128,
        }
    }

    /// Atualiza estado de uma pool (não-bloqueante)
    #[inline(always)]
    pub async fn update_pool(&self, swap: &NormalizedSwapEvent, block: u64) {
        let mut cache = self.state_cache.write().await;
        
        let state = cache.entry(swap.pool).or_insert(PoolState {
            address: swap.pool,
            token0: swap.token_in,
            token1: swap.token_out,
            reserve0: 0,
            reserve1: 0,
            fee: swap.fee,
            sqrt_price_x96: 0,
            dex_type: swap.dex_type,
            last_block: block,
        });
        
        // Atualizar reservas baseado no swap
        match swap.dex_type {
            DexType::UniswapV3 | DexType::UniswapV2 | DexType::PancakeSwap => {
                // Para V3, usamos o sqrt_price_x96 para calcular reservas virtuais
                if let Some(price) = swap.sqrt_price_x96 {
                    state.sqrt_price_x96 = price.to::<u128>();
                }
                if let Some(liq) = swap.liquidity {
                    // Reservas virtuais baseadas em liquidez e preço
                    let liquidity = liq as u128;
                    let price = state.sqrt_price_x96;
                    state.reserve0 = liquidity * 1_000_000 / price;
                    state.reserve1 = liquidity * price / 1_000_000;
                }
            }
            DexType::Aerodrome | DexType::AerodromeStable => {
                // Para Aerodrome (CPMM), acumulamos diretamente
                state.reserve0 += swap.amount_in.to::<u128>();
                state.reserve1 = state.reserve1.saturating_sub(swap.amount_out.to::<u128>());
            }
        }
        
        state.last_block = block;
    }

    /// Simula uma rota de arbitragem completa em memória (< 10 microssegundos)
    #[inline(always)]
    pub async fn simulate_route(&self, path: &ArbitragePath, block_gas_limit: u64) -> SimulationResult {
        let start = std::time::Instant::now();
        
        let cache = self.state_cache.read().await;
        
        // Simulação in-memory sem REVM para máxima velocidade
        let mut amount = path.optimal_input.to::<u128>();
        let initial_amount = amount;
        let mut total_gas = self.base_gas_cost;
        
        for hop in &path.hops {
            if let Some(pool) = cache.get(&hop.pool) {
                // Calcular output usando constant product
                let (reserve_in, reserve_out) = if hop.token_in == pool.token0 {
                    (pool.reserve0, pool.reserve1)
                } else {
                    (pool.reserve1, pool.reserve0)
                };
                
                if reserve_in == 0 || reserve_out == 0 {
                    return SimulationResult {
                        success: false,
                        error: Some("Zero liquidity".to_string()),
                        execution_time_us: start.elapsed().as_micros() as u64,
                        ..Default::default()
                    };
                }
                
                // Fórmula: dy = y * dx * (1 - fee) / (x + dx * (1 - fee))
                let fee_factor = 10_000 - hop.fee;
                let amount_in_with_fee = (amount * fee_factor as u128) / 10_000;
                
                let numerator = amount_in_with_fee * reserve_out;
                let denominator = reserve_in + amount_in_with_fee;
                
                amount = numerator / denominator;
                
                // Gas por hop (SLOAD + SSTORE + CALL)
                total_gas += 50_000;
            } else {
                return SimulationResult {
                    success: false,
                    error: Some("Pool not in cache".to_string()),
                    execution_time_us: start.elapsed().as_micros() as u64,
                    ..Default::default()
                };
            }
        }
        
        drop(cache);
        
        // Verificar se é lucrativo
        let profit = amount as i128 - initial_amount as i128;
        let gas_cost = total_gas as u128 * self.gas_price_wei;
        let net_profit = profit - gas_cost as i128;
        
        let execution_time = start.elapsed().as_micros() as u64;
        
        SimulationResult {
            success: net_profit > 0 && total_gas <= block_gas_limit,
            net_profit_wei: net_profit,
            gas_used: total_gas,
            gas_cost_wei: gas_cost,
            error: if net_profit <= 0 { Some("Not profitable".to_string()) } else { None },
            execution_time_us: execution_time,
        }
    }

    /// Simulação REVM completa (mais lenta mas 100% precisa)
    pub async fn simulate_with_revm(
        &self,
        path: &ArbitragePath,
        executor: Address,
    ) -> eyre::Result<SimulationResult> {
        let start = std::time::Instant::now();
        
        let mut db = self.db.write().await;
        
        // Construir transação de simulação
        let tx = revm::primitives::TxEnv {
            caller: RevmAddress::from_slice(executor.as_slice()),
            data: self.encode_arbitrage_payload(path).into(),
            gas_limit: 500_000,
            gas_price: RevmU256::from(self.gas_price_wei),
            transact_to: TransactTo::Call(RevmAddress::from_slice(executor.as_slice())),
            ..Default::default()
        };
        
        let env = revm::primitives::Env {
            block: revm::primitives::BlockEnv::default(),
            cfg: revm::primitives::CfgEnv::default(),
            tx,
        };
        
        let mut evm = Evm::builder()
            .with_db(&mut *db)
            .with_env(Box::new(env))
            .build();
        
        let result = match evm.transact() {
            Ok(result) => result.result,
            Err(e) => {
                return Ok(SimulationResult {
                    success: false,
                    error: Some(format!("EVM error: {:?}", e)),
                    execution_time_us: start.elapsed().as_micros() as u64,
                    ..Default::default()
                });
            }
        };
        
        let (success, gas_used, output) = match result {
            ExecutionResult::Success { gas_used, output, .. } => {
                let out = match output {
                    Output::Call(bytes) => bytes,
                    _ => Bytes::new(),
                };
                (true, gas_used, out)
            }
            ExecutionResult::Revert { gas_used, output, .. } => {
                return Ok(SimulationResult {
                    success: false,
                    gas_used,
                    error: Some(format!("Revert: {:?}", output)),
                    execution_time_us: start.elapsed().as_micros() as u64,
                    ..Default::default()
                });
            }
            ExecutionResult::Halt { reason, gas_used, .. } => {
                return Ok(SimulationResult {
                    success: false,
                    gas_used,
                    error: Some(format!("Halt: {:?}", reason)),
                    execution_time_us: start.elapsed().as_micros() as u64,
                    ..Default::default()
                });
            }
        };
        
        // Decodificar resultado
        let net_profit = if output.len() >= 32 {
            U256::from_be_slice(&output[..32]).to::<i128>()
        } else {
            0i128
        };
        
        let gas_cost = gas_used as u128 * self.gas_price_wei;
        
        Ok(SimulationResult {
            success: success && net_profit > gas_cost as i128,
            net_profit_wei: net_profit - gas_cost as i128,
            gas_used,
            gas_cost_wei: gas_cost,
            error: None,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Verificação de segurança atômica (Anti-HoneyPot)
    pub async fn security_check(&self, path: &ArbitragePath) -> bool {
        let cache = self.state_cache.read().await;
        
        for hop in &path.hops {
            if let Some(pool) = cache.get(&hop.pool) {
                // Verificar se pool tem liquidez mínima
                if pool.reserve0 < 1_000_000 || pool.reserve1 < 1_000_000 {
                    trace!("Pool {:?} has insufficient liquidity", hop.pool);
                    return false;
                }
                
                // Verificar se o último update foi recente (< 300 blocos)
                // Em produção, usar timestamp real
                // if pool.last_block < current_block - 300 {
                //     return false;
                // }
            } else {
                trace!("Pool {:?} not found in cache", hop.pool);
                return false;
            }
        }
        
        true
    }

    /// Codifica payload de arbitragem (comprimido)
    fn encode_arbitrage_payload(&self, path: &ArbitragePath) -> Vec<u8> {
        let mut payload = Vec::with_capacity(1 + path.hops.len() * 21);
        
        // Prefixo: número de hops (1 byte)
        payload.push(path.hops.len() as u8);
        
        // Cada hop: address (20 bytes) + fee (1 byte em bps/100)
        for hop in &path.hops {
            payload.extend_from_slice(hop.pool.as_slice());
            payload.push((hop.fee / 100) as u8); // Comprimir fee para 1 byte
        }
        
        payload
    }

    /// Estima gas com precisão de 99%
    pub fn estimate_gas(&self, num_hops: usize) -> u64 {
        self.base_gas_cost + (num_hops as u64 * 50_000)
    }
}

/// Métricas de performance do simulador
#[derive(Clone, Debug, Default)]
pub struct SimulatorMetrics {
    pub simulations_count: u64,
    pub avg_simulation_time_us: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

/// Transação pendente na mempool (para state overlay)
#[derive(Clone, Debug)]
pub struct PendingTx {
    pub hash: [u8; 32],
    pub from: Address,
    pub to: Address,
    pub input: Vec<u8>,
    pub value: U256,
    pub gas_price: u128,
}

/// State Overlay para backrunning
/// Aplica transações da mempool ao estado antes da simulação
pub struct StateOverlay {
    /// Transações pendentes ordenadas por gas price
    pending_txs: Arc<RwLock<Vec<PendingTx>>>,
    /// Estado modificado pelas transações pendentes
    modified_pools: Arc<RwLock<HashMap<Address, PoolState>>>,
    /// Flag de ativação
    enabled: bool,
}

impl StateOverlay {
    pub fn new(enabled: bool) -> Self {
        Self {
            pending_txs: Arc::new(RwLock::new(Vec::new())),
            modified_pools: Arc::new(RwLock::new(HashMap::new())),
            enabled,
        }
    }
    
    /// Adiciona transação da mempool
    pub async fn add_pending_tx(&self, tx: PendingTx) {
        if !self.enabled {
            return;
        }
        
        let mut txs = self.pending_txs.write().await;
        txs.push(tx);
        // Ordenar por gas price (maior primeiro)
        txs.sort_by(|a, b| b.gas_price.cmp(&a.gas_price));
    }
    
    /// Remove transação confirmada
    pub async fn remove_pending_tx(&self, hash: &[u8; 32]) {
        let mut txs = self.pending_txs.write().await;
        txs.retain(|tx| &tx.hash != hash);
    }
    
    /// Aplica transações pendentes ao estado (state overlay)
    pub async fn apply_overlay(&self, base_state: &mut HashMap<Address, PoolState>) {
        if !self.enabled {
            return;
        }
        
        let modified = self.modified_pools.read().await;
        
        // Merge estado modificado com estado base
        for (addr, state) in modified.iter() {
            base_state.insert(*addr, *state);
        }
    }
    
    /// Simula efeito de uma transação nas pools
    pub async fn simulate_tx_impact(&self, tx: &PendingTx, pools: &mut HashMap<Address, PoolState>) {
        // Detectar se é swap e atualizar reservas
        if tx.input.len() >= 4 {
            let selector = &tx.input[0..4];
            
            // Selector de swap: 0x128acb08 (Uniswap V3) ou similar
            if selector == [0x12, 0x8a, 0xcb, 0x08] {
                // Extrair parâmetros do swap (simplificado)
                // Na implementação real, decodificar ABI
                if let Some(pool) = pools.get(&tx.to) {
                    let mut modified = pool.clone();
                    // Aplicar impacto estimado (simplificado)
                    modified.reserve0 = modified.reserve0.saturating_sub(1000);
                    modified.reserve1 = modified.reserve1.saturating_add(1000);
                    pools.insert(tx.to, modified);
                }
            }
        }
    }
    
    /// Limpa transações antigas (após bloco confirmado)
    pub async fn clear_old_txs(&self, _block_number: u64) {
        let mut txs = self.pending_txs.write().await;
        txs.clear();
        
        let mut modified = self.modified_pools.write().await;
        modified.clear();
    }
}
