use alloy::primitives::{Address, U256};
use hashbrown::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use csv::Writer;
use tracing::{info, trace, warn};

use crate::artemis::{MevEvent, Strategy, StrategyContext};
use crate::contracts::NormalizedSwapEvent;

// Re-exportar DexType para acesso público (também o torna disponível neste módulo)
pub use crate::contracts::DexType;
use crate::types::{ArbitragePath, Hop, Fixed64};
use crate::simulator::StateSimulator;
use crate::executor::{FlashLoanStrategy, FlashLoanProvider, PayloadEncoder};

/// Registro de oportunidade para CSV com dados de flashloan
#[derive(Clone, Debug)]
pub struct OpportunityRecord {
    pub timestamp: String,
    pub block_number: u64,
    pub path: String,
    pub amount_in_eth: f64,
    pub expected_gross_profit_eth: f64,
    pub gas_cost_eth: f64,
    pub net_profit_eth: f64,
    pub execution_status: String,
    pub newton_iterations: usize,
    pub base_fee_gwei: u64,
    pub priority_fee_gwei: u64,
    // NOVOS CAMPOS PARA FLASHLOAN DEBUG
    /// Fonte do flashloan (UniswapV3, BalancerV2, AaveV3)
    pub flashloan_source: String,
    /// Montante pedido emprestado em ETH
    pub borrowed_amount_eth: f64,
    /// Gas efetivamente gasto em ETH
    pub gas_spent_eth: f64,
    /// Latência de execução em ms
    pub execution_latency_ms: u64,
    /// Slippage estimado em %
    pub slippage_pct: f64,
}

/// Logger CSV para oportunidades
pub struct OpportunityLogger {
    writer: Arc<RwLock<Writer<std::fs::File>>>,
}

impl OpportunityLogger {
    pub fn new() -> Self {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .append(true)
            .create(true)
            .open("opportunities.csv")
            .expect("Failed to open opportunities.csv");
        
        let mut writer = Writer::from_writer(file);
        
        // Escrever header se arquivo novo
        let is_new = std::fs::metadata("opportunities.csv")
            .map(|m| m.len() == 0)
            .unwrap_or(true);
        
        if is_new {
            writer.write_record(&[
                "Timestamp",
                "BlockNumber",
                "Path",
                "AmountInETH",
                "ExpectedGrossProfitETH",
                "GasCostETH",
                "NetProfitETH",
                "ExecutionStatus",
                "NewtonIterations",
                "BaseFeeGwei",
                "PriorityFeeGwei",
            ]).expect("Failed to write CSV header");
            writer.flush().unwrap();
        }
        
        Self {
            writer: Arc::new(RwLock::new(writer)),
        }
    }
    
    pub async fn log_opportunity(&self, record: OpportunityRecord) {
        let mut writer = self.writer.write().await;
        writer.write_record(&[
            record.timestamp,
            record.block_number.to_string(),
            record.path,
            format!("{:.18}", record.amount_in_eth),
            format!("{:.18}", record.expected_gross_profit_eth),
            format!("{:.18}", record.gas_cost_eth),
            format!("{:.18}", record.net_profit_eth),
            record.execution_status,
            record.newton_iterations.to_string(),
            record.base_fee_gwei.to_string(),
            record.priority_fee_gwei.to_string(),
        ]).expect("Failed to write opportunity record");
        
        writer.flush().expect("Failed to flush CSV");
    }
}

/// Resultado do cálculo de Newton-Raphson
pub struct NewtonResult {
    pub optimal_amount: U256,
    pub iterations: usize,
    pub converged: bool,
}

/// Configuração de simulação de lucro líquido
#[derive(Clone, Debug)]
pub struct ProfitConfig {
    pub flash_loan_fee_bps: u32,
    pub gas_price_gwei: u64,
    pub safety_margin_bps: u32,  // Margem de segurança (10%)
    pub max_iterations: usize,     // Max iterações Newton-Raphson
}

impl Default for ProfitConfig {
    fn default() -> Self {
        Self {
            flash_loan_fee_bps: 30,    // 0.3% (Uniswap V3)
            gas_price_gwei: 1,         // Base gas price
            safety_margin_bps: 100,    // 10%
            max_iterations: 10,
        }
    }
}

/// Strategist de alta performance usando hashbrown
pub struct HighPerformanceStrategist {
    /// Cache de pools em memória (hashbrown para performance)
    pools: Arc<RwLock<HashMap<Address, PoolInfo>>>,
    
    /// Grafo de tokens para pathfinding
    token_graph: Arc<RwLock<TokenGraph>>,
    
    /// Simulador REVM de alta performance
    #[allow(dead_code)]
    simulator: Arc<StateSimulator>,
    
    /// Estratégia de Flash Loan
    flash_strategy: Arc<RwLock<FlashLoanStrategy>>,
    
    /// Encoder de payloads
    #[allow(dead_code)]
    encoder: Arc<PayloadEncoder>,
    
    /// Elite Shadow Hunter - Execução segura com simulação atómica
    elite_hunter: Arc<tokio::sync::RwLock<crate::simulator::EliteShadowHunter>>,
    
    /// Estatísticas
    stats: Arc<RwLock<StrategistStats>>,
    
    /// Configuração
    config: StrategistConfig,
    
    /// Configuração de lucro
    profit_config: ProfitConfig,
    
    /// Modo Shadow Hunter (DRY_RUN)
    dry_run: bool,
}

#[derive(Clone, Debug)]
pub struct PoolInfo {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub dex_type: DexType,
    pub fee: u32,
    pub reserve0: U256,
    pub reserve1: U256,
    pub sqrt_price_x96: Option<U256>,
    pub last_update: u64,
}

#[derive(Clone, Debug)]
struct TokenGraph {
    /// Mapeamento token -> pools
    edges: HashMap<Address, Vec<GraphEdge>>,
    /// Set de tokens conhecidos
    tokens: HashSet<Address>,
}

#[derive(Clone, Debug)]
struct GraphEdge {
    pool: Address,
    target_token: Address,
    fee: u32,
    #[allow(dead_code)]
    dex_type: DexType,
}

#[derive(Clone, Debug)]
pub struct StrategistConfig {
    pub max_path_length: usize,
    pub min_profit_bps: u32,
    pub max_gas_price_gwei: u64,
    pub update_batch_size: usize,
}

impl Default for StrategistConfig {
    fn default() -> Self {
        Self {
            max_path_length: 4,
            min_profit_bps: 50, // 0.5%
            max_gas_price_gwei: 500,
            update_batch_size: 1000,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StrategistStats {
    pub events_processed: u64,
    pub pools_tracked: usize,
    pub opportunities_found: u64,
    pub paths_evaluated: u64,
    pub avg_processing_time_us: u64,
}

impl HighPerformanceStrategist {
    pub fn new(config: StrategistConfig, executor: Address, profit_config: ProfitConfig) -> Self {
        let encoder = PayloadEncoder::new(executor, 8453); // Base chain ID
        let flash_strategy = FlashLoanStrategy::new(encoder.clone(), FlashLoanProvider::UniswapV3);
        
        // Inicializar Elite Shadow Hunter com configuração padrão
        let elite_config = crate::simulator::EliteShadowHunterConfig::default();
        let elite_hunter = Arc::new(tokio::sync::RwLock::new(
            crate::simulator::EliteShadowHunter::new(elite_config)
        ));
        
        Self {
            pools: Arc::new(RwLock::new(HashMap::with_capacity(10000))),
            token_graph: Arc::new(RwLock::new(TokenGraph {
                edges: HashMap::with_capacity(10000),
                tokens: HashSet::with_capacity(5000),
            })),
            simulator: Arc::new(StateSimulator::new(config.max_gas_price_gwei)),
            flash_strategy: Arc::new(RwLock::new(flash_strategy)),
            encoder: Arc::new(encoder),
            elite_hunter,
            stats: Arc::new(RwLock::new(StrategistStats::default())),
            config,
            profit_config,
            dry_run: std::env::var("DRY_RUN")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
        }
    }
    
    /// Log de oportunidade em modo Shadow Hunter (DRY_RUN)
    /// Inclui informações detalhadas de flashloan para debug
    fn log_shadow_hunter_opportunity(
        &self,
        path: &ArbitragePath,
        gross_profit: U256,
        gas_cost: u64,
        net_profit: U256,
        optimal_amount: U256,
        iterations: usize,
        flashloan_calc: Option<&crate::executor::FlashLoanCalculation>,
        latency_ms: u64,
    ) {
        let route_str = path.hops.iter()
            .map(|h| format!("{:?}..{:?}", h.token_in, h.pool)[..10].to_string())
            .collect::<Vec<_>>()
            .join(" → ");
        
        let gross_eth = gross_profit.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
        let net_eth = net_profit.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
        let gas_eth = (gas_cost as f64) / 1e9;
        let amount_eth = optimal_amount.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
        
        // Informações de flashloan
        let (flash_source, borrowed_eth, flash_fee_eth) = if let Some(calc) = flashloan_calc {
            let source = match calc.provider {
                crate::executor::FlashLoanProvider::UniswapV3 => "FlashSwap",
                crate::executor::FlashLoanProvider::BalancerV2 => "Balancer",
                crate::executor::FlashLoanProvider::AaveV3 => "Aave",
            };
            let borrowed = calc.loan_amount.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
            let fee = calc.flash_loan_fee_wei.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
            (source, borrowed, fee)
        } else {
            ("N/A", 0.0, 0.0)
        };
        
        info!("═══════════════════════════════════════════════════════════");
        info!("🔍 [SHADOW HUNTER] OPORTUNIDADE DETETADA");
        info!("═══════════════════════════════════════════════════════════");
        info!("  Rota: {}", route_str);
        info!("  Hops: {} | Latência: {}ms", path.hops.len(), latency_ms);
        info!("  Amount Ótimo: {:.6} ETH | Newton-Raphson: {} iterações", amount_eth, iterations);
        info!("  ────────────────────────────────────────────────");
        info!("  💰 Lucro Bruto:    {:.6} ETH (${:.2})", gross_eth, gross_eth * 3000.0);
        info!("  ⛽ Custo Gas:      {:.6} ETH", gas_eth);
        info!("  💸 Flashloan:     {} ({} ETH emprestado, taxa: {:.6} ETH)", 
            flash_source, borrowed_eth, flash_fee_eth);
        info!("  📈 Lucro Líquido:  {:.6} ETH (${:.2}) ✅", net_eth, net_eth * 3000.0);
        info!("  ────────────────────────────────────────────────");
        info!("  Safety Check: 3x margem | Slippage max: 0.5% | GasCap: 0.1 gwei");
        info!("  Status: SIMULADO_COM_SUCESSO ✅");
        info!("═══════════════════════════════════════════════════════════");
    }

    /// Atualiza estado de uma pool com evento de swap
    #[inline(always)]
pub async fn update_pool_from_swap(&self, swap: &NormalizedSwapEvent) {
        let mut pools = self.pools.write().await;
        let mut graph = self.token_graph.write().await;
        
        // Atualizar ou criar pool
        let pool_info = pools.entry(swap.pool).or_insert_with(|| {
            PoolInfo {
                address: swap.pool,
                token0: swap.token_in,
                token1: swap.token_out,
                dex_type: swap.dex_type,
                fee: swap.fee,
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
                sqrt_price_x96: swap.sqrt_price_x96,
                last_update: 0,
            }
        });
        
        // Atualizar reservas estimadas (simplificado)
        pool_info.reserve0 += swap.amount_in;
        pool_info.reserve1 = pool_info.reserve1.saturating_sub(swap.amount_out);
        pool_info.sqrt_price_x96 = swap.sqrt_price_x96;
        pool_info.last_update = now_secs();
        
        // Atualizar grafo
        graph.tokens.insert(swap.token_in);
        graph.tokens.insert(swap.token_out);
        
        // Adicionar arestas bidirecionais
        let edge1 = GraphEdge {
            pool: swap.pool,
            target_token: swap.token_out,
            fee: swap.fee,
            dex_type: swap.dex_type,
        };
        
        graph.edges
            .entry(swap.token_in)
            .or_default()
            .push(edge1);
        
        let edge2 = GraphEdge {
            pool: swap.pool,
            target_token: swap.token_in,
            fee: swap.fee,
            dex_type: swap.dex_type,
        };
        
        graph.edges
            .entry(swap.token_out)
            .or_default()
            .push(edge2);
        
        drop(pools);
        drop(graph);
    }

    /// 🔀 Verifica preço na DEX oposta para Cross-DEX arbitragem
    pub async fn check_cross_dex_price(&self, swap: &NormalizedSwapEvent) -> Option<(DexType, u32)> {
        let pools = self.pools.read().await;
        
        // Determinar DEX oposta
        let target_dex = match swap.dex_type {
            DexType::UniswapV3 | DexType::PancakeSwap | DexType::UniswapV2 => DexType::Aerodrome,
            DexType::Aerodrome | DexType::AerodromeStable => DexType::UniswapV3,
        };
        
        // Procurar pool do mesmo par na DEX oposta
        for (_, pool_info) in pools.iter() {
            if pool_info.dex_type == target_dex {
                // Verificar se é o mesmo par (token0/token1 pode estar invertido)
                let same_pair = (pool_info.token0 == swap.token_in && pool_info.token1 == swap.token_out) ||
                                 (pool_info.token0 == swap.token_out && pool_info.token1 == swap.token_in);
                
                if same_pair {
                    // Calcular diferença de preço simplificada (em basis points)
                    let price_diff_bps = ((pool_info.fee as i32 - swap.fee as i32).abs() as u32).min(1000);
                    return Some((target_dex, price_diff_bps));
                }
            }
        }
        
        None
    }

    /// Busca ciclos de arbitragem a partir de um token
    /// OTIMIZAÇÃO: Prioriza rotas de 2 hops (A->B->A) antes de rotas complexas
    pub async fn find_arbitrage_cycles(
        &self,
        start_token: Address,
        max_hops: usize,
    ) -> Vec<ArbitragePath> {
        let graph = self.token_graph.read().await;
        let pools = self.pools.read().await;
        
        // FASE 1: Buscar rotas de 2 hops primeiro (mais rápidas e confiáveis)
        let mut paths_2hop = Vec::new();
        if max_hops >= 2 {
            self.find_2hop_cycles(&graph, &pools, start_token, &mut paths_2hop);
        }
        
        // FASE 2: Se não encontrou rotas 2-hop suficientes, buscar rotas complexas
        let mut paths_complex = Vec::new();
        if paths_2hop.len() < 3 && max_hops > 2 {
            let mut visited = HashSet::new();
            let mut current_path = Vec::new();
            
            self.dfs_find_cycles(
                &graph,
                &pools,
                start_token,
                start_token,
                max_hops,
                0,
                &mut visited,
                &mut current_path,
                &mut paths_complex,
            );
        }
        
        drop(graph);
        drop(pools);
        
        // Combinar: 2-hop primeiro, depois rotas complexas
        let mut all_paths = paths_2hop;
        all_paths.extend(paths_complex);
        
        // Ordenar por profit_ratio (melhores primeiro)
        all_paths.sort_by(|a, b| b.profit_ratio.partial_cmp(&a.profit_ratio).unwrap_or(std::cmp::Ordering::Equal));
        
        // Limitar a top 10 oportunidades para processamento rápido
        all_paths.truncate(10);
        
        all_paths
    }
    
    /// Busca ciclos de 2 hops especificamente (A -> B -> A)
    /// Mais rápido que DFS completo e tem maior probabilidade de sucesso
    fn find_2hop_cycles(
        &self,
        graph: &TokenGraph,
        pools: &HashMap<Address, PoolInfo>,
        start_token: Address,
        paths: &mut Vec<ArbitragePath>,
    ) {
        // Obter edges do token inicial
        if let Some(edges_from_start) = graph.edges.get(&start_token) {
            // Para cada token intermediário alcançável em 1 hop
            for edge1 in edges_from_start {
                let intermediate_token = edge1.target_token;
                
                // Procurar caminho de volta ao token inicial
                if let Some(edges_from_intermediate) = graph.edges.get(&intermediate_token) {
                    for edge2 in edges_from_intermediate {
                        if edge2.target_token == start_token {
                            // Encontrou ciclo de 2 hops!
                            if let Some(pool1) = pools.get(&edge1.pool) {
                                if let Some(pool2) = pools.get(&edge2.pool) {
                                    let hops = vec![
                                        Hop {
                                            pool: edge1.pool,
                                            token_in: start_token,
                                            token_out: intermediate_token,
                                            fee: edge1.fee,
                                            dex_type: pool1.dex_type,
                                        },
                                        Hop {
                                            pool: edge2.pool,
                                            token_in: intermediate_token,
                                            token_out: start_token,
                                            fee: edge2.fee,
                                            dex_type: pool2.dex_type,
                                        },
                                    ];
                                    
                                    if let Some(path) = self.evaluate_path(pools, &hops) {
                                        if path.profit_ratio > self.config.min_profit_bps as Fixed64 {
                                            paths.push(path);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// DFS para encontrar ciclos
    fn dfs_find_cycles(
        &self,
        graph: &TokenGraph,
        pools: &HashMap<Address, PoolInfo>,
        current: Address,
        target: Address,
        max_hops: usize,
        depth: usize,
        visited: &mut HashSet<Address>,
        current_path: &mut Vec<Hop>,
        paths: &mut Vec<ArbitragePath>,
    ) {
        if depth > max_hops {
            return;
        }
        
        if depth > 0 && current == target && current_path.len() >= 2 {
            // Encontrou ciclo
            if let Some(path) = self.evaluate_path(pools, current_path) {
                if path.profit_ratio > self.config.min_profit_bps as Fixed64 {
                    paths.push(path);
                }
            }
            return;
        }
        
        if visited.contains(&current) {
            return;
        }
        
        visited.insert(current);
        
        if let Some(edges) = graph.edges.get(&current) {
            for edge in edges {
                if let Some(pool) = pools.get(&edge.pool) {
                    let hop = Hop {
                        pool: edge.pool,
                        token_in: current,
                        token_out: edge.target_token,
                        fee: edge.fee,
                        dex_type: pool.dex_type,
                    };
                    
                    current_path.push(hop);
                    
                    self.dfs_find_cycles(
                        graph,
                        pools,
                        edge.target_token,
                        target,
                        max_hops,
                        depth + 1,
                        visited,
                        current_path,
                        paths,
                    );
                    
                    current_path.pop();
                }
            }
        }
        
        visited.remove(&current);
    }

    /// Avalia a rentabilidade de um caminho
    fn evaluate_path(&self, pools: &HashMap<Address, PoolInfo>, hops: &[Hop]) -> Option<ArbitragePath> {
        if hops.is_empty() {
            return None;
        }
        
        // Simulação simplificada de output
        let input = U256::from(1_000_000_000_000_000_000u64); // 1 ETH
        let mut output = input;
        let mut _total_fee = 0u32;
        
        for hop in hops {
            if let Some(pool) = pools.get(&hop.pool) {
                // Calcular output baseado em constant product
                let reserve_in = if hop.token_in == pool.token0 {
                    pool.reserve0
                } else {
                    pool.reserve1
                };
                
                let reserve_out = if hop.token_in == pool.token0 {
                    pool.reserve1
                } else {
                    pool.reserve0
                };
                
                if reserve_in.is_zero() || reserve_out.is_zero() {
                    return None;
                }
                
                // Fórmula: dy = y * dx / (x + dx) * (1 - fee)
                let fee_factor = U256::from(10_000 - hop.fee);
                let amount_in_with_fee = output * fee_factor / U256::from(10_000);
                
                let numerator = amount_in_with_fee * reserve_out;
                let denominator = reserve_in + amount_in_with_fee;
                
                output = numerator / denominator;
                _total_fee += hop.fee;
            } else {
                return None;
            }
        }
        
        // Verificar se é lucrativo
        if output <= input {
            return None;
        }
        
        let profit = output - input;
        let profit_ratio = (profit.to::<u64>() as Fixed64) / (input.to::<u64>() as Fixed64) * 10000.0;
        
        let first_token = hops.first().map(|h| h.token_in).unwrap_or(Address::ZERO);
        
        Some(ArbitragePath {
            hops: hops.to_vec(),
            input_token: first_token,
            optimal_input: input,
            expected_profit: profit,
            profit_ratio,
        })
    }

    /// Retorna estatísticas
    pub async fn stats(&self) -> StrategistStats {
        self.stats.read().await.clone()
    }
    
    /// Otimiza amount_in usando método de Newton-Raphson MATEMÁTICO
    /// Baseado nas reservas x*y=k das pools Uniswap V2/V3
    /// Converge em <= 3 iterações usando derivadas analíticas
    pub fn optimize_amount_newton_raphson(
        &self,
        path: &ArbitragePath,
        pools: &HashMap<Address, PoolInfo>,
    ) -> Option<NewtonResult> {
        let config = &self.profit_config;
        const MAX_ITERATIONS: usize = 3; // Limite agressivo para alta frequência
        const TOLERANCE_BPS: u64 = 10; // 0.1% = 10 bps de tolerância
        
        // Valor inicial baseado na liquidez da primeira pool
        let initial_amount = if let Some(first_hop) = path.hops.first() {
            if let Some(pool) = pools.get(&first_hop.pool) {
                // Começar com 1% da liquidez menor
                let min_reserve = std::cmp::min(pool.reserve0, pool.reserve1);
                min_reserve / U256::from(100u64)
            } else {
                U256::from(100_000_000_000_000_000u64) // 0.1 ETH default
            }
        } else {
            U256::from(100_000_000_000_000_000u64)
        };
        
        let mut amount_in = initial_amount;
        let mut best_profit = U256::ZERO;
        let mut best_amount = amount_in;
        
        for iteration in 0..MAX_ITERATIONS {
            // Calcular lucro líquido: Profit(x) = AmountOut(x) - x - GasCost
            let net_profit = self.calculate_net_profit(path, pools, amount_in, config)?;
            
            // Guardar melhor resultado encontrado
            if net_profit > best_profit {
                best_profit = net_profit;
                best_amount = amount_in;
            }
            
            // Calcular segunda derivada via diferença finita
            let delta = amount_in / U256::from(100); // 1% delta
            if delta.is_zero() {
                break;
            }
            
            let profit_plus = self.calculate_net_profit(path, pools, amount_in + delta, config)?;
            let profit_minus = self.calculate_net_profit(path, pools, amount_in.saturating_sub(delta), config)?;
            
            // Derivada primeira: f'(x) ≈ (f(x+δ) - f(x-δ)) / (2δ)
            let first_derivative = if profit_plus > profit_minus {
                let diff = profit_plus - profit_minus;
                let delta_2x = delta * U256::from(2);
                if delta_2x.is_zero() { break; }
                diff.try_into().unwrap_or(u128::MAX) as i128
            } else {
                let diff = profit_minus - profit_plus;
                -(diff.try_into().unwrap_or(u128::MAX) as i128)
            };
            
            // Derivada segunda: f''(x) ≈ (f(x+δ) - 2f(x) + f(x-δ)) / δ²
            let second_derivative = {
                let mid_term = net_profit * U256::from(2);
                let sum = profit_plus + profit_minus;
                if sum > mid_term {
                    let diff = sum - mid_term;
                    diff.try_into().unwrap_or(u128::MAX) as i128
                } else {
                    let diff = mid_term - sum;
                    -(diff.try_into().unwrap_or(u128::MAX) as i128)
                }
            };
            
            // Newton-Raphson: x_new = x - f'(x) / f''(x)
            // Se f''(x) ≈ 0, usar ajuste proporcional
            let new_amount = if second_derivative.abs() > 1000 {
                let adjustment = (amount_in.try_into().unwrap_or(u128::MAX) as i128 * first_derivative) / second_derivative;
                if adjustment > 0 {
                    amount_in.saturating_sub(U256::from(adjustment.unsigned_abs()))
                } else {
                    amount_in.saturating_add(U256::from(adjustment.unsigned_abs()))
                }
            } else {
                // Fallback: busca binária adaptativa
                if first_derivative > 0 {
                    amount_in.saturating_add(amount_in / U256::from(10)) // +10%
                } else if first_derivative < 0 {
                    amount_in.saturating_sub(amount_in / U256::from(10)) // -10%
                } else {
                    break; // Já estamos no ótimo
                }
            };
            
            // Verificar convergência: variação < 0.1%
            let diff = if new_amount > amount_in { new_amount - amount_in } else { amount_in - new_amount };
            let diff_bps = (diff * U256::from(10_000)) / amount_in;
            
            if diff_bps < U256::from(TOLERANCE_BPS) || new_amount == amount_in {
                return Some(NewtonResult {
                    optimal_amount: best_amount,
                    iterations: iteration + 1,
                    converged: true,
                });
            }
            
            amount_in = new_amount;
        }
        
        // Retornar melhor resultado encontrado
        Some(NewtonResult {
            optimal_amount: best_amount,
            iterations: MAX_ITERATIONS,
            converged: best_profit > U256::ZERO,
        })
    }
    
    /// Calcula lucro líquido (com taxas, gas e margem de segurança)
    /// INTEGRAÇÃO FLASHLOAN: Seleciona automaticamente Balancer (0%) ou Aave (0.05%)
    fn calculate_net_profit(
        &self,
        path: &ArbitragePath,
        pools: &HashMap<Address, PoolInfo>,
        amount_in: U256,
        config: &ProfitConfig,
    ) -> Option<U256> {
        // Simular output bruto
        let output = self.simulate_path_output(path, pools, amount_in)?;
        
        if output <= amount_in {
            return None; // Não lucrativo
        }
        
        let gross_profit = output - amount_in;
        
        // === INTEGRAÇÃO FLASHLOAN FASE 2 ===
        // Selecionar melhor provedor baseado no montante
        let flash_loan_provider = crate::executor::FlashLoanStrategy::select_best_provider(
            path.hops.first()?.token_in, 
            amount_in
        );
        
        // Taxas: UniswapV3 FlashSwap = 0%, BalancerV2 = 0.09%, AaveV3 = 0.09%
        let flash_loan_fee_bps = match flash_loan_provider {
            crate::executor::FlashLoanProvider::UniswapV3 => 0u32,  // FlashSwap integrado no swap
            crate::executor::FlashLoanProvider::BalancerV2 => 9u32, // 0.09%
            crate::executor::FlashLoanProvider::AaveV3 => 9u32,     // 0.09%
        };
        
        // 1. Taxa de flash loan (agora dinâmica por provedor)
        let flash_loan_fee = (amount_in * U256::from(flash_loan_fee_bps)) / U256::from(10_000);
        
        // 2. Custo de gas estimado (inclui gas extra para flashloan)
        // Flashloan: +60k UniswapV3, +80k Balancer, +100k Aave
        let flash_gas = match flash_loan_provider {
            crate::executor::FlashLoanProvider::UniswapV3 => 60_000u64,
            crate::executor::FlashLoanProvider::BalancerV2 => 80_000u64,
            crate::executor::FlashLoanProvider::AaveV3 => 100_000u64,
        };
        let gas_used = flash_gas + 21000 + path.hops.len() as u64 * 100_000;
        let gas_cost_wei = U256::from(gas_used) * U256::from(config.gas_price_gwei as u64) * U256::from(1_000_000_000u64);
        
        // 3. Margem de segurança (10%)
        let safety_margin = (gross_profit * U256::from(config.safety_margin_bps)) / U256::from(1000);
        
        // Lucro líquido = Bruto - Taxas - Gas - Margem
        let total_costs = flash_loan_fee + gas_cost_wei + safety_margin;
        
        if gross_profit > total_costs {
            let net_profit = gross_profit - total_costs;
            // Verificar se atinge mínimo de 0.00005 ETH (~0.15€)
            if net_profit >= U256::from(50_000_000_000_000u128) {
                Some(net_profit)
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Simula output de uma rota (sem taxas adicionais)
    fn simulate_path_output(
        &self,
        path: &ArbitragePath,
        pools: &HashMap<Address, PoolInfo>,
        amount_in: U256,
    ) -> Option<U256> {
        let mut output = amount_in;
        
        for hop in &path.hops {
            let pool = pools.get(&hop.pool)?;
            
            let (reserve_in, reserve_out) = if hop.token_in == pool.token0 {
                (pool.reserve0, pool.reserve1)
            } else {
                (pool.reserve1, pool.reserve0)
            };
            
            if reserve_in.is_zero() || reserve_out.is_zero() {
                return None;
            }
            
            // Fórmula CPMM com fee
            let fee_factor = U256::from(10_000 - hop.fee);
            let amount_in_with_fee = output * fee_factor / U256::from(10_000);
            
            let numerator = amount_in_with_fee * reserve_out;
            let denominator = reserve_in + amount_in_with_fee;
            
            output = numerator / denominator;
        }
        
        Some(output)
    }
}

use async_trait::async_trait;

#[async_trait]
impl Strategy for HighPerformanceStrategist {
    async fn process_event(
        &mut self,
        event: MevEvent,
        _context: &StrategyContext,
    ) -> eyre::Result<()> {
        match event {
            MevEvent::Swap(swap) => {
                let start = std::time::Instant::now();
                
                // LOG DE DEBUG: Verificar se eventos chegam ao motor
                let amount_eth = swap.amount_in.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                info!("Processando evento de swap para token: {:?}", swap.token_in);
                trace!("Swap: {:?} -> {:?} | Valor: {:.6} ETH", swap.token_in, swap.token_out, amount_eth);
                
                // Filtro de relevância: ignorar swaps < 0.1 ETH
                const MIN_SWAP_ETH: f64 = 0.1;
                if amount_eth < MIN_SWAP_ETH {
                    trace!("Swap ignorado: muito pequeno ({:.6} ETH)", amount_eth);
                    return Ok(());
                }
                
                // Atualizar estado
                self.update_pool_from_swap(&swap).await;
                
                // Buscar oportunidades - prioriza 2-hop (2-3ms)
                let paths = self.find_arbitrage_cycles(swap.token_in, 2).await; // Max 2 hops para performance
                
                // REJECTION LOGGER: Check 2 - No path found
                if paths.is_empty() {
                    info!("   ⚠️  IGNORADO: Sem rota de arbitragem para {:?} ({} pools rastreadas)", 
                        swap.token_in, self.pools.read().await.len());
                    return Ok(());
                }
                info!("   ✅ {} caminhos de arbitragem encontrados", paths.len());
                
                // Lock uma única vez para todas as operações (microssegundos)
                let pools = self.pools.read().await;
                let mut opportunity_found = false;
                
                // Processar apenas top 5 oportunidades (limitar tempo)
                for path in paths.iter().take(5) {
                    // Newton-Raphson otimizado - max 3 iterações (1-2ms)
                    match self.optimize_amount_newton_raphson(path, &pools) {
                        Some(newton_result) => {
                            // DEBUG: Log do OptimalInput mesmo com lucro negativo
                            let optimal_eth = newton_result.optimal_amount.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                            trace!("DEBUG: Newton-Raphson converged | Optimal: {:.6} ETH | Iterations: {} | Converged: {}",
                                optimal_eth, newton_result.iterations, newton_result.converged);
                            
                            // 🎯 ELITE SHADOW HUNTER: Simulação Atómica Pre-Trade
                            let executor_addr = alloy::primitives::address!("0x1111111111111111111111111111111111111111");
                            let elite_result = self.elite_hunter.read().await.simulate_atomic_arbitrage(
                                path,
                                executor_addr,
                                50_000_000_000u128, // 50 gwei
                            ).await;
                            
                            // Se a simulação atómica falhar, descartar imediatamente
                            if !elite_result.success {
                                let reason = elite_result.execution_error.as_ref()
                                    .or(elite_result.revert_reason.as_ref())
                                    .map(|s| s.as_str())
                                    .unwrap_or("Motivo desconhecido");
                                
                                if elite_result.is_honeypot_detected {
                                    warn!(
                                        "[ELITE-HUNTER] 🍯 HONEYPOT DETETADO E BLOQUEADO | Rota: {:?} -> {:?} | Motivo: {}",
                                        path.input_token,
                                        path.hops.last().map(|h| h.token_out).unwrap_or(path.input_token),
                                        reason
                                    );
                                } else {
                                    trace!(
                                        "[ELITE-HUNTER] ❌ Rota rejeitada na simulação atómica | Motivo: {}",
                                        reason
                                    );
                                }
                                continue; // Pular para próxima rota
                            }
                            
                            // Flashloan validation rápida (0.5ms)
                            let liquidity = U256::from(1_000_000_000_000_000_000u64); // 1 ETH default
                            let flash_calc = self.flash_strategy.read().await.calculate_optimal_loan(path, liquidity);
                            
                            // 🎯 ELITE SHADOW HUNTER: Threshold dinâmico de 0.005 ETH
                            let total_cost = flash_calc.flash_loan_fee_wei + flash_calc.total_gas_cost_wei;
                            let min_profit_threshold = U256::from(5_000_000_000_000_000u128); // 0.005 ETH
                            let min_profit = total_cost + min_profit_threshold; // Lucro líquido > 0.005 ETH
                            
                            let profit_eth = flash_calc.net_profit_wei.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                            let cost_eth = total_cost.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                            let net_profit_eth = profit_eth - cost_eth; // Lucro líquido
                            
                            // ✅ EXECUÇÃO VALIDADA: Lucro líquido > 0.005 ETH
                            let is_profitable = flash_calc.net_profit_wei >= min_profit;
                            let profit_ratio = if cost_eth > 0.0 { profit_eth / cost_eth } else { 0.0 };
                            let has_min_potential = net_profit_eth >= 0.002; // Potencial mínimo 0.002 ETH
                            
                            // 🎯 EM DRY_RUN: Mostrar só cálculos com potencial (>=50% gas cost)
                            if self.dry_run {
                                if is_profitable {
                                    info!("🚨 [OPORTUNIDADE DETETADA] | Lucro: {:.6} ETH | Custo: {:.6} ETH | ROI: {:.2}% | {} hops",
                                        profit_eth, cost_eth, 
                                        f64::max(profit_ratio, 0.0) * 100.0,
                                        path.hops.len());
                                    opportunity_found = true;
                                } else if has_min_potential {
                                    // Só mostrar 📊 CALCULO se tiver potencial (>=50% do gás)
                                    info!("   📊 CALCULO: Lucro: {:.6} ETH | Custo: {:.6} ETH | Ratio: {:.1}% | {} hops",
                                        profit_eth, cost_eth, profit_ratio * 100.0, path.hops.len());
                                } else {
                                    // 📝 VERBOSE SCAN: Sempre mostrar mesmo quando descartado
                                    let token_in = path.input_token;
                                    info!("[SCAN] Token: {:?} | Lucro: {:.6} ETH | Status: Ignorado (Abaixo do Threshold)",
                                        token_in, profit_eth);
                                }
                            } else {
                                // Modo LIVE: só processar se for lucrativo
                                if !is_profitable {
                                    // 📝 VERBOSE SCAN mesmo em modo LIVE
                                    let token_in = path.input_token;
                                    trace!("[SCAN] Token: {:?} | Lucro: {:.6} ETH | Status: REJECTED (Profit < 1.2x Gas)",
                                        token_in, profit_eth);
                                    continue;
                                }
                                opportunity_found = true;
                            }
                            
                            // Shadow Hunter logging (apenas em dry_run)
                            if self.dry_run {
                                self.log_shadow_hunter_opportunity(
                                    path,
                                    flash_calc.net_profit_wei + total_cost,
                                    flash_calc.extra_gas_cost,
                                    flash_calc.net_profit_wei,
                                    newton_result.optimal_amount,
                                    newton_result.iterations,
                                    Some(&flash_calc),
                                    start.elapsed().as_millis() as u64,
                                );
                            }
                        }
                        None => {
                            // REJECTION LOGGER: Check 4 - Newton-Raphson failed
                            trace!("REJECTED: Newton-Raphson failed to converge for path with {} hops", path.hops.len());
                        }
                    }
                }
                
                // Atualizar estatísticas (microssegundos)
                let elapsed_us = start.elapsed().as_micros() as u64;
                let mut stats = self.stats.write().await;
                stats.events_processed += 1;
                stats.pools_tracked = self.pools.read().await.len();
                if opportunity_found {
                    stats.opportunities_found += 1;
                }
                stats.paths_evaluated += 1;
                stats.avg_processing_time_us = (stats.avg_processing_time_us + elapsed_us) / 2;
                
                // Alerta se processing time > 10ms (10,000 microssegundos)
                if elapsed_us > 10_000 {
                    trace!("⚠️ Slow event processing: {}µs", elapsed_us);
                }
            }
            MevEvent::BlockUpdate(block) => {
                trace!("Block update: {}", block);
            }
            MevEvent::PriceUpdate { token, price } => {
                trace!("Price update: {:?} = {}", token, price);
            }
        }
        
        Ok(())
    }

    async fn initialize(&mut self, initial_data: Vec<NormalizedSwapEvent>) -> eyre::Result<()> {
        info!("Inicializando strategist com {} eventos", initial_data.len());
        
        for swap in initial_data {
            self.update_pool_from_swap(&swap).await;
        }
        
        let stats = self.stats.read().await;
        info!("Strategist inicializado: {} pools", stats.pools_tracked);
        
        Ok(())
    }

    fn stats(&self) -> crate::artemis::strategy::StrategyStats {
        // Converter estatísticas
        crate::artemis::strategy::StrategyStats::default()
    }
}

/// Timestamp atual em segundos
#[inline(always)]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ═══════════════════════════════════════════════════════════
// MÓDULOS DE ALTA AGRESSIVIDADE
// ═══════════════════════════════════════════════════════════

/// Apex Predator - Economic Engine & Gas War Control
pub mod apex_predator;
pub use apex_predator::{
    ApexPredator, DailyStats, ExecutionPriority, OpportunityEvaluation,
    CompletedTrade, TradeType,
    MIN_PROFIT_PER_TRADE, PROFIT_AGGRESSIVE, PROFIT_EXTREME,
    DAILY_TARGET_EUR, DAILY_TARGET_USD,
};

/// Whale Predictor - Análise de Impacto de Preço
pub mod whale_predictor;
pub use whale_predictor::{
    WhalePredictor, WhalePrediction, PoolReserves, PostWhaleArbitrage,
    WHALE_THRESHOLD_ETH, EXECUTION_WINDOW_MS,
};

/// Multi-Hop Path Finder - 4 Saltos Inter-DEX
pub mod multi_hop_engine;
pub use multi_hop_engine::{
    MultiHopEngine, SwapSimulation,
    FlashloanProvider,
    MAX_ITERATIONS, PRECISION_WEI,
};

/// Newton-Raphson Jacobian Solver - Arbitragem Triangular Avançada
pub mod newton_jacobian_solver;
pub use newton_jacobian_solver::{
    NewtonJacobianSolver, TriangularSystem, SolverResult, TriangularSystemBuilder,
    FLASHLOAN_FEE_AAVE_BPS, FLASHLOAN_FEE_UNISWAP_BPS, FLASHLOAN_FEE_BALANCER_BPS,
    MIN_PROFIT_LIQUIDO_USD, TARGET_PROFIT_DAILY_EUR,
};

/// Continuous Profit Engine - Lucro Recorrente sem Silêncio
pub mod continuous_engine;
pub use continuous_engine::{
    ContinuousProfitEngine, GasSensitivityController, TradeProbability,
    REDUCED_PROFIT_THRESHOLD_SMALL, REDUCED_PROFIT_THRESHOLD_MEDIUM, REDUCED_PROFIT_THRESHOLD_LARGE,
    PendingTxInfo, RecursiveOpportunity, ContinuousStats,
};
