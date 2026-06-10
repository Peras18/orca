//! MULTI-HOP PATH FINDER - 4 Saltos Inter-DEX
//! 
//! Motor de cálculo avançado com Newton-Raphson para rotas complexas:
//! WETH -> USDC (UniV3) -> AERO (Aero) -> WETH (UniV3)
//! 
//! Target: 3000€/dia com alavancagem de $100k+

use alloy::primitives::{Address, U256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{info, debug};

use crate::types::{PoolReserves, DexType};

/// 🎯 MULTI-HOP PATH ENGINE (4 Saltos Máximo)
pub struct MultiHopEngine {
    /// Grafo de pools (token -> [(pool, token_out)])
    pool_graph: Arc<RwLock<HashMap<Address, Vec<(Address, Address, DexType)>>>>,
    
    /// Reservas das pools
    reserves: Arc<RwLock<HashMap<Address, PoolReserves>>>,
    
    /// Taxas de swap por DEX (em basis points)
    dex_fees: HashMap<DexType, u32>,
    
    /// Limite de slippage máximo (1%)
    max_slippage_bps: u32, // 100 = 1%
}

/// 🛤️ Caminho de Arbitragem Multi-Hop (até 4 hops)
#[derive(Clone, Debug)]
pub struct MultiHopArbitragePath {
    /// Sequência de pools [pool1, pool2, pool3, pool4]
    pub pools: Vec<Address>,
    /// Sequência de tokens [token0, token1, token2, token3, token4=token0]
    pub tokens: Vec<Address>,
    /// DEX de cada hop
    pub dexes: Vec<DexType>,
    /// Quantidade inicial ótima (calculada por Newton-Raphson)
    pub optimal_amount_in: U256,
    /// Lucro esperado em USD
    pub expected_profit_usd: f64,
    /// Slippage estimado total
    pub total_slippage_bps: u32,
    /// Número de hops (2-4)
    pub hop_count: u8,
    /// Score de prioridade (lucro / tempo)
    pub priority_score: f64,
}

/// 📊 Resultado de Simulação de Swap
#[derive(Clone, Debug)]
pub struct SwapSimulation {
    pub amount_out: U256,
    pub price_impact_bps: f32,
    pub fee_paid: U256,
    pub effective_price: f64,
}

/// 🔢 Constantes Newton-Raphson
pub const MAX_ITERATIONS: u32 = 10;
pub const PRECISION_WEI: u128 = 1_000_000_000_000_000; // 0.001 ETH precision
const INITIAL_GUESS_ETH: f64 = 1.0; // Começar com 1 ETH

/// 🔄 Flashloan Integration para Multi-Hop
pub struct FlashloanMultiHop {
    /// Fontes de flashloan
    providers: Vec<FlashloanProvider>,
    /// Máximo por transação
    max_amount_per_tx: U256,
}

#[derive(Clone, Debug)]
pub enum FlashloanProvider {
    AaveV3,      // 0.09% fee
    BalancerV2,  // 0% fee
    UniswapV3,   // 0.3% fee (flash swap)
}

impl MultiHopEngine {
    pub fn new() -> Self {
        let mut dex_fees = HashMap::new();
        dex_fees.insert(DexType::UniswapV3, 5);   // 0.05%
        dex_fees.insert(DexType::UniswapV2, 30);  // 0.3%
        dex_fees.insert(DexType::Aerodrome, 30);   // 0.3%
        
        Self {
            pool_graph: Arc::new(RwLock::new(HashMap::new())),
            reserves: Arc::new(RwLock::new(HashMap::new())),
            dex_fees,
            max_slippage_bps: 100, // 1% max
        }
    }
    
    /// 🚀 Inicializa o motor
    pub async fn initialize(&self, pools: Vec<(Address, Address, Address, DexType)>) {
        let mut graph = self.pool_graph.write().await;
        let mut reserves = self.reserves.write().await;
        
        for (pool, token0, token1, dex) in &pools {
            // Adicionar ao grafo (bidirecional)
            graph.entry(*token0).or_insert_with(Vec::new).push((*pool, *token1, *dex));
            graph.entry(*token1).or_insert_with(Vec::new).push((*pool, *token0, *dex));
            
            // Inicializar reservas (serão atualizadas em runtime)
            reserves.insert(*pool, PoolReserves {
                token0: *token0,
                token1: *token1,
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
                fee: self.dex_fees.get(dex).copied().unwrap_or(30),
            });
        }
        
        info!("🛤️🛤️🛤️ [MULTI-HOP] Grafo construído: {} pools, {} tokens conectados",
            pools.len(), graph.len());
    }
    
    /// 🎯 Encontra todos os caminhos de arbitragem (2-4 hops)
    pub async fn find_arbitrage_paths(
        &self,
        start_token: Address,
        max_hops: u8,
        min_profit_usd: f64,
    ) -> Vec<MultiHopArbitragePath> {
        let start = Instant::now();
        let mut paths = Vec::new();
        
        // Busca em profundidade limitada (DFS)
        self.dfs_find_cycles(
            start_token,
            start_token,
            vec![start_token],
            vec![],
            vec![],
            max_hops,
            &mut paths,
            min_profit_usd,
        ).await;
        
        // Ordenar por lucro esperado
        paths.sort_by(|a, b| b.expected_profit_usd.partial_cmp(&a.expected_profit_usd).unwrap());
        
        let elapsed = start.elapsed().as_micros();
        info!("🛤️ [MULTI-HOP] {} caminhos encontrados em {}µs", paths.len(), elapsed);
        
        paths
    }
    
    /// 🔍 DFS para encontrar ciclos de arbitragem
    async fn dfs_find_cycles(
        &self,
        start: Address,
        current: Address,
        tokens_visited: Vec<Address>,
        pools_path: Vec<Address>,
        dexes_path: Vec<DexType>,
        remaining_hops: u8,
        results: &mut Vec<MultiHopArbitragePath>,
        min_profit: f64,
    ) {
        if remaining_hops == 0 {
            return;
        }
        
        let graph = self.pool_graph.read().await;
        
        // Verificar se podemos voltar ao início
        if pools_path.len() >= 2 {
            if let Some(connections) = graph.get(&current) {
                for (pool, next_token, dex) in connections {
                    if *next_token == start {
                        // Ciclo completo encontrado!
                        let mut full_tokens = tokens_visited.clone();
                        full_tokens.push(start);
                        
                        let mut full_pools = pools_path.clone();
                        full_pools.push(*pool);
                        
                        let mut full_dexes = dexes_path.clone();
                        full_dexes.push(*dex);
                        
                        // Calcular oportunidade
                        if let Some(path) = self.calculate_path_profit(
                            full_pools,
                            full_tokens,
                            full_dexes,
                            min_profit,
                        ).await {
                            results.push(path);
                        }
                        break;
                    }
                }
            }
        }
        
        // Continuar DFS se não atingimos max_hops
        if remaining_hops > 1 {
            if let Some(connections) = graph.get(&current) {
                for (pool, next_token, dex) in connections {
                    // Evitar ciclos internos
                    if !tokens_visited.contains(next_token) {
                        let mut new_tokens = tokens_visited.clone();
                        new_tokens.push(*next_token);
                        
                        let mut new_pools = pools_path.clone();
                        new_pools.push(*pool);
                        
                        let mut new_dexes = dexes_path.clone();
                        new_dexes.push(*dex);
                        
                        // Recursão assíncrona
                        Box::pin(self.dfs_find_cycles(
                            start,
                            *next_token,
                            new_tokens,
                            new_pools,
                            new_dexes,
                            remaining_hops - 1,
                            results,
                            min_profit,
                        )).await;
                    }
                }
            }
        }
    }
    
    /// 🧮 Calcula lucro de um caminho usando Newton-Raphson
    async fn calculate_path_profit(
        &self,
        pools: Vec<Address>,
        tokens: Vec<Address>,
        dexes: Vec<DexType>,
        min_profit: f64,
    ) -> Option<MultiHopArbitragePath> {
        let hop_count = pools.len() as u8;
        
        // Newton-Raphson: Encontrar amount_in ótimo
        let optimal_amount = self.newton_raphson_optimize(&pools, &tokens).await?;
        
        // Simular path completo
        let (final_amount, total_slippage) = self.simulate_path(
            &pools, &tokens, optimal_amount
        ).await?;
        
        // Calcular lucro
        let profit = final_amount.saturating_sub(optimal_amount);
        let profit_eth = profit.to::<u128>() as f64 / 1e18;
        let profit_usd = profit_eth * 2500.0; // ETH @ $2500
        
        if profit_usd < min_profit {
            return None;
        }
        
        // Calcular score de prioridade (lucro / hops)
        let priority_score = profit_usd / hop_count as f64;
        
        Some(MultiHopArbitragePath {
            pools,
            tokens,
            dexes,
            optimal_amount_in: optimal_amount,
            expected_profit_usd: profit_usd,
            total_slippage_bps: total_slippage as u32,
            hop_count,
            priority_score,
        })
    }
    
    /// 🔢 Newton-Raphson: Otimização de amount_in para lucro MÁXIMO
    /// 
    /// Fórmula: x_{n+1} = x_n - P'(x_n)/P''(x_n)
    /// Onde P(x) = output(x) - x (Lucro Bruto)
    async fn newton_raphson_optimize(
        &self,
        pools: &[Address],
        tokens: &[Address],
    ) -> Option<U256> {
        // Chute inicial: 1 ETH
        let mut x_f = 1.0; 
        let h = 0.001; // 1 finney
        
        for iteration in 0..MAX_ITERATIONS {
            // P(x) = simulate(x) - x
            let u_val_center = U256::from((x_f * 1e18) as u128);
            let (out_center, _) = self.simulate_path(pools, tokens, u_val_center).await.unwrap_or((U256::ZERO, 0.0));
            let p_center = out_center.to::<u128>() as f64 / 1e18 - x_f;

            let u_val_plus = U256::from(((x_f + h) * 1e18) as u128);
            let (out_plus, _) = self.simulate_path(pools, tokens, u_val_plus).await.unwrap_or((U256::ZERO, 0.0));
            let p_plus = out_plus.to::<u128>() as f64 / 1e18 - (x_f + h);

            let u_val_minus = U256::from(((x_f - h) * 1e18) as u128);
            let (out_minus, _) = self.simulate_path(pools, tokens, u_val_minus).await.unwrap_or((U256::ZERO, 0.0));
            let p_minus = out_minus.to::<u128>() as f64 / 1e18 - (x_f - h);

            // Derivadas
            let p_prime = (p_plus - p_minus) / (2.0 * h);
            let p_double_prime = (p_plus - 2.0 * p_center + p_minus) / (h * h);

            if p_double_prime.abs() < 1e-9 {
                break;
            }

            let delta = p_prime / p_double_prime;
            x_f -= delta;

            // Constraints: Mínimo 0.01 ETH, Máximo 40 ETH ($100k)
            if x_f < 0.01 { x_f = 0.01; }
            if x_f > 40.0 { x_f = 40.0; }

            if delta.abs() < 0.0001 {
                debug!("🎯 [NEWTON-MAX] Convergiu em {} iterações: {:.4} ETH", iteration, x_f);
                break;
            }
        }
        
        Some(U256::from((x_f * 1e18) as u128))
    }
    
    /// 🔄 Simula um path completo de swaps
    async fn simulate_path(
        &self,
        pools: &[Address],
        tokens: &[Address],
        amount_in: U256,
    ) -> Option<(U256, f64)> {
        let mut amount = amount_in;
        let mut total_slippage = 0.0;
        let reserves = self.reserves.read().await;
        
        for (i, pool) in pools.iter().enumerate() {
            let token_in = tokens[i];
            let _token_out = tokens[i + 1];
            
            let pool_reserves = reserves.get(pool)?;
            
            // Simular swap nesta pool
            let (reserve_in, reserve_out) = if token_in == pool_reserves.token0 {
                (pool_reserves.reserve0, pool_reserves.reserve1)
            } else {
                (pool_reserves.reserve1, pool_reserves.reserve0)
            };
            
            if reserve_in.is_zero() || reserve_out.is_zero() {
                return None; // Pool vazia
            }
            
            // Fórmula Uniswap V2: amount_out = (amount_in * 997 * reserve_out) / (reserve_in * 1000 + amount_in * 997)
            let fee_factor = 10000 - pool_reserves.fee;
            let amount_in_with_fee = amount * U256::from(fee_factor) / U256::from(10000);
            
            let numerator = amount_in_with_fee * reserve_out;
            let denominator = reserve_in + amount_in_with_fee;
            
            if denominator.is_zero() {
                return None;
            }
            
            amount = numerator / denominator;
            
            // Calcular slippage deste hop
            let price_before = reserve_out.to::<u128>() as f64 / reserve_in.to::<u128>() as f64;
            let new_reserve_in = reserve_in + amount_in_with_fee;
            let new_reserve_out = reserve_out - amount;
            let price_after = new_reserve_out.to::<u128>() as f64 / new_reserve_in.to::<u128>() as f64;
            let slippage = ((price_after - price_before) / price_before).abs() * 10000.0; // bps
            total_slippage += slippage;
        }
        
        Some((amount, total_slippage))
    }
    
    /// 📊 Atualiza reservas de uma pool (chamar a cada bloco)
    pub async fn update_pool_reserves(
        &self,
        pool: Address,
        reserve0: U256,
        reserve1: U256,
    ) {
        let mut reserves = self.reserves.write().await;
        if let Some(r) = reserves.get_mut(&pool) {
            r.reserve0 = reserve0;
            r.reserve1 = reserve1;
        }
    }
    
    /// 🎯 Executa busca completa e retorna top 10 oportunidades
    pub async fn find_top_opportunities(
        &self,
        start_token: Address,
        min_profit_usd: f64,
    ) -> Vec<MultiHopArbitragePath> {
        let mut all_paths = Vec::new();
        
        // Buscar caminhos de 2, 3 e 4 hops
        for hops in 2..=4 {
            let paths = self.find_arbitrage_paths(start_token, hops, min_profit_usd).await;
            all_paths.extend(paths);
        }
        
        // Ordenar por lucro e retornar top 10
        all_paths.sort_by(|a, b| b.expected_profit_usd.partial_cmp(&a.expected_profit_usd).unwrap());
        all_paths.truncate(10);
        
        info!("🎯🎯🎯 [MULTI-HOP] Top {} oportunidades encontradas", all_paths.len());
        for (i, path) in all_paths.iter().enumerate() {
            info!("    #{}: {} hops | ${:.2} | Score: {:.1}", 
                i + 1, path.hop_count, path.expected_profit_usd, path.priority_score);
        }
        
        all_paths
    }
}


impl FlashloanMultiHop {
    pub fn new() -> Self {
        Self {
            providers: vec![
                FlashloanProvider::BalancerV2, // Preferir 0% fee
                FlashloanProvider::AaveV3,
                FlashloanProvider::UniswapV3,
            ],
            max_amount_per_tx: U256::from(100_000_000_000_000_000_000_000u128), // $100k @ ETH=$2500
        }
    }
    
    /// 💰 Calcula quantidade máxima de flashloan para um path
    pub fn calculate_optimal_flashloan(
        &self,
        path: &MultiHopArbitragePath,
        available_liquidity: U256,
    ) -> U256 {
        // Limitar ao mínimo entre: max_per_tx, available, optimal_amount * 10
        let ten_x_optimal = path.optimal_amount_in * U256::from(10);
        
        let max = self.max_amount_per_tx.min(available_liquidity).min(ten_x_optimal);
        
        info!("💰💰💰 [FLASHLOAN] Quantidade alavancada: {} ETH (${})",
            max.to::<u128>() as f64 / 1e18,
            max.to::<u128>() as f64 / 1e18 * 2500.0);
        
        max
    }
    
    /// Seleciona melhor provider baseado na fee
    pub fn select_best_provider(&self, _amount: U256) -> (FlashloanProvider, U256) {
        // Balancer: 0% fee (melhor)
        (FlashloanProvider::BalancerV2, U256::ZERO)
    }
}
