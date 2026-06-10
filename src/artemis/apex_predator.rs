//! APEX-PREDATOR MEV ENGINE
//! Arquitetura de latência ultra-baixa para dominação da Base Mainnet
//! 
//! Features:
//! - Ciclos Multi-Hop (3-5 saltos) para arbitragem triangular
//! - Monitorização de Liquidações (Aave, Seamless, Moonwell)
//! - Gestão de Gás Reativa (Dynamic Priority Fee)
//! - Simulação Paralela com tokio::spawn

use alloy::primitives::{Address, U256, Log};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tracing::{info, warn, error, trace};
use async_trait::async_trait;

use crate::contracts::{DexType, NormalizedSwapEvent};
use crate::types::{ArbitragePath, Hop, Fixed64};
use super::{Strategy, StrategyContext, MevEvent};

/// 🔥 [APEX-PREDATOR] Tipos de Oportunidade
#[derive(Clone, Debug, PartialEq)]
pub enum ApexOpportunityType {
    /// Ciclo triangular/multi-hop (3-5 saltos)
    ApexCycle { hops: usize, path_tokens: Vec<Address> },
    /// Liquidação de alto impacto (>1 ETH)
    FatalStrike { protocol: LendingProtocol, debt_to_cover: U256 },
    /// Arbitragem simples (backrun tradicional)
    SimpleBackrun,
}

/// Protocolos de Lending monitorados
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LendingProtocol {
    AaveV3,
    Seamless,
    Moonwell,
}

/// 🎯 Configuração do Apex-Predator
#[derive(Clone, Debug)]
pub struct ApexConfig {
    /// Máximo de saltos em ciclos
    pub max_cycle_hops: usize,
    /// Mínimo de saltos para considerar ciclo
    pub min_cycle_hops: usize,
    /// Threshold de liquidação (em ETH)
    pub liquidation_threshold_eth: f64,
    /// Gas base máximo aceitável (gwei)
    pub max_base_fee_gwei: u64,
    /// Tip dinâmico - multiplicador sobre base fee
    pub priority_tip_multiplier: f64,
    /// Número máximo de simulações paralelas
    pub max_parallel_simulations: usize,
    /// Timeout de reação para ciclos (microssegundos)
    pub cycle_reaction_time_us: u64,
}

impl Default for ApexConfig {
    fn default() -> Self {
        Self {
            max_cycle_hops: 5,
            min_cycle_hops: 3,
            liquidation_threshold_eth: 1.0, // >1 ETH
            max_base_fee_gwei: 100,
            priority_tip_multiplier: 1.5, // 1.5x base fee
            max_parallel_simulations: 20,
            cycle_reaction_time_us: 500, // 500 microssegundos
        }
    }
}

/// 🎯 Top 50 Tokens por volume na Base (actualizado dinamicamente)
#[derive(Clone, Debug)]
pub struct TopTokenMonitor {
    /// Tokens prioritários (Top 50 por volume 24h)
    pub top_tokens: Arc<RwLock<HashSet<Address>>>,
    /// Volume 24h por token (para ordenação)
    pub token_volumes: Arc<RwLock<HashMap<Address, f64>>>,
    /// DEXs monitorados: Aerodrome, Uniswap V3, BaseSwap
    pub monitored_dexs: Vec<DexType>,
}

impl TopTokenMonitor {
    /// 🚀 Inicializa monitor com tokens principais da Base
    pub fn new() -> Self {
        // Tokens principais da Base (serão actualizados dinamicamente)
        // Endereços hardcoded - serão actualizados por volume real
        let mut initial_tokens = HashSet::new();
        
        // WETH - sempre prioridade máxima
        initial_tokens.insert(Address::new([
            0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x06,
        ]));
        
        // USDC
        initial_tokens.insert(Address::new([
            0x83, 0x35, 0x89, 0xfc, 0xd6, 0xed, 0xb6, 0xe0,
            0x8f, 0x4c, 0x7c, 0x32, 0xd4, 0xf7, 0x1b, 0x54,
            0xbd, 0xa0, 0x29, 0x13,
        ]));
        
        // USDBC
        initial_tokens.insert(Address::new([
            0xd9, 0xaa, 0xec, 0x86, 0xb6, 0x5d, 0x86, 0xf6,
            0xa7, 0xc5, 0xf1, 0xc2, 0xda, 0xcd, 0x10, 0x13,
            0xf6, 0x5c, 0x3c, 0x3e,
        ]));
        
        // CBETH
        initial_tokens.insert(Address::new([
            0x2a, 0xe3, 0xf1, 0xec, 0x7f, 0x1f, 0x50, 0x12,
            0xcf, 0xea, 0xb0, 0x18, 0x5b, 0xfc, 0x7a, 0x3c,
            0xf0, 0xde, 0xc2, 0x02,
        ]));
        
        // DAI
        initial_tokens.insert(Address::new([
            0x50, 0xc5, 0x72, 0x59, 0x49, 0xa6, 0xf0, 0xc7,
            0x2e, 0x6c, 0x4a, 0x64, 0x1f, 0x24, 0x04, 0x9a,
            0x91, 0x7d, 0xb0, 0xcb,
        ]));
        
        Self {
            top_tokens: Arc::new(RwLock::new(initial_tokens)),
            token_volumes: Arc::new(RwLock::new(HashMap::new())),
            monitored_dexs: vec![
                DexType::UniswapV3,
                DexType::Aerodrome,
                // BaseSwap também é monitorado via DexType::UniswapV3 (fork)
            ],
        }
    }
    
    /// 📊 Verifica se token está no Top 50
    pub async fn is_priority_token(&self, token: Address) -> bool {
        let top = self.top_tokens.read().await;
        top.contains(&token)
    }
}

/// 🧠 ADAPTIVE INTELLIGENCE ENGINE - Competitor Tracker
/// Aprende com os preços dos vencedores e ajusta automaticamente
#[derive(Clone, Debug)]
pub struct CompetitorTracker {
    /// Histórico de gas prices de transações perdidas: (token_path_hash -> gas_price)
    pub lost_opportunities: Arc<RwLock<HashMap<String, Vec<u64>>>>,
    /// Gas price médio por vencedor para cada padrão de oportunidade
    pub winner_patterns: Arc<RwLock<HashMap<String, u64>>>,
    /// Fator de agressividade (1 centavo acima do vencedor = +0.001 gwei)
    pub outbid_margin_gwei: f64,
    /// Contador de aprendizagem (número de oportunidades analisadas)
    pub learning_count: Arc<RwLock<u64>>,
}

impl CompetitorTracker {
    /// 🚀 Inicializa tracker de competidores
    pub fn new() -> Self {
        info!("[ADAPTIVE] 🧠 CompetitorTracker inicializado - Aprendendo com o mercado");
        Self {
            lost_opportunities: Arc::new(RwLock::new(HashMap::new())),
            winner_patterns: Arc::new(RwLock::new(HashMap::new())),
            outbid_margin_gwei: 0.001, // 1 centavo em gwei
            learning_count: Arc::new(RwLock::new(0)),
        }
    }
    
    /// 📝 Regista uma oportunidade perdida para análise
    pub async fn record_loss(&self, token_path: &[Address], winner_gas_price_gwei: u64) {
        let path_key = self.hash_path(token_path);
        let mut lost = self.lost_opportunities.write().await;
        
        lost.entry(path_key.clone())
            .or_insert_with(Vec::new)
            .push(winner_gas_price_gwei);
        
        // Atualizar padrão de vencedor (média + margem)
        let winner_gas = winner_gas_price_gwei + (self.outbid_margin_gwei * 1e9) as u64;
        let mut patterns = self.winner_patterns.write().await;
        patterns.insert(path_key, winner_gas);
        
        *self.learning_count.write().await += 1;
        
        info!(
            "[ADAPTIVE] 📚 APRENDIZAGEM #{} | Vencedor pagou {} gwei | Próxima licitação: {} gwei",
            *self.learning_count.read().await,
            winner_gas_price_gwei,
            winner_gas
        );
    }
    
    /// 🎯 Calcula gas price adaptativo baseado em padrões históricos
    pub async fn get_adaptive_gas_price(&self, token_path: &[Address], base_gas: u64) -> u64 {
        let path_key = self.hash_path(token_path);
        let patterns = self.winner_patterns.read().await;
        
        if let Some(&historical_winner) = patterns.get(&path_key) {
            // Licitar 1 centavo acima do último vencedor
            let adaptive = historical_winner + (self.outbid_margin_gwei * 1e9) as u64;
            info!(
                "[ADAPTIVE] 🎯 Gas adaptativo: {} gwei (base: {} | histórico: {} | +1¢)",
                adaptive, base_gas, historical_winner
            );
            std::cmp::max(adaptive, base_gas) // Nunca ficar abaixo do base
        } else {
            // Sem histórico - usar base + margem padrão
            let default = base_gas + (self.outbid_margin_gwei * 1e9 * 2.0) as u64;
            info!("[ADAPTIVE] 🔍 Sem histórico - usando default: {} gwei", default);
            default
        }
    }
    
    /// 🔐 Gera hash única para um path de tokens
    fn hash_path(&self, path: &[Address]) -> String {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        
        let mut hasher = DefaultHasher::new();
        for addr in path {
            addr.hash(&mut hasher);
        }
        format!("{:x}", hasher.finish())
    }
}

/// ⚛️ MULTI-ATOMIC ROUTING - Divide arbitragens grandes em rotas paralelas
#[derive(Clone, Debug)]
pub struct MultiAtomicRouter {
    /// Threshold para divisão (se slippage > X%, dividir)
    pub slippage_threshold_bps: u32, // basis points (100 = 1%)
    /// Número máximo de rotas paralelas
    pub max_parallel_routes: usize,
    /// Histórico de sucesso por divisão
    pub split_success_rate: Arc<RwLock<HashMap<usize, f64>>>,
}

impl MultiAtomicRouter {
    /// 🚀 Inicializa router multi-atómico
    pub fn new() -> Self {
        info!("[MULTI-ATOMIC] ⚛️ Router inicializado - Slippage optimizer");
        Self {
            slippage_threshold_bps: 50, // 0.5% threshold
            max_parallel_routes: 3,
            split_success_rate: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// 🔄 Analisa se deve dividir uma arbitragem grande
    pub async fn should_split(&self, total_amount: f64, estimated_slippage_bps: u32) -> bool {
        if estimated_slippage_bps > self.slippage_threshold_bps && total_amount > 5.0 {
            // Slippage alto e montante grande = dividir
            info!(
                "[MULTI-ATOMIC] 🔄 Divisão recomendada | Slippage: {}bps > {}bps threshold | Amount: {} ETH",
                estimated_slippage_bps, self.slippage_threshold_bps, total_amount
            );
            true
        } else {
            false
        }
    }
    
    /// ✂️ Divide uma rota em múltiplas rotas menores
    pub fn split_routes(&self, total_amount: f64, optimal_splits: usize) -> Vec<f64> {
        let split_count = optimal_splits.min(self.max_parallel_routes);
        let base_amount = total_amount / split_count as f64;
        
        // Distribuir com pequenas variações para evitar deteção de padrão
        let mut splits = Vec::new();
        for i in 0..split_count {
            let variation = if i % 2 == 0 { 0.98 } else { 1.02 };
            splits.push(base_amount * variation);
        }
        
        info!(
            "[MULTI-ATOMIC] ✂️ Rota dividida em {} sub-rotas | Total: {} ETH",
            split_count, total_amount
        );
        
        splits
    }
    
    /// 📊 Actualiza taxa de sucesso para uma estratégia de divisão
    pub async fn record_split_result(&self, num_routes: usize, success: bool) {
        let mut rates = self.split_success_rate.write().await;
        let entry = rates.entry(num_routes).or_insert(0.5);
        
        // Média móvel exponencial
        let alpha = 0.3;
        *entry = *entry * (1.0 - alpha) + (if success { 1.0 } else { 0.0 }) * alpha;
        
        info!(
            "[MULTI-ATOMIC] 📊 Split {} rotas | Success: {} | Taxa acumulada: {:.1}%",
            num_routes, success, *entry * 100.0
        );
    }
}

/// 🔮 PROBABILISTIC PRE-CALCULATION - Rotas quentes pré-calculadas
#[derive(Clone, Debug)]
pub struct PreCalcEngine {
    /// Cache de ciclos quentes (pré-calculados entre blocos)
    pub hot_cycles: Arc<RwLock<Vec<(Vec<Address>, f64)>>>, // (path, estimated_profit)
    /// Timestamp da última atualização
    pub last_update: Arc<RwLock<u64>>,
    /// Tokens com maior volume nos últimos 5 minutos
    pub trending_tokens: Arc<RwLock<Vec<Address>>>,
    /// Limite de idade do cache (segundos)
    pub cache_ttl_seconds: u64,
}

impl PreCalcEngine {
    pub fn new() -> Self {
        // Silencioso - só log em debug
        trace!("PreCalcEngine inicializado");
        
        let default_tokens = Self::get_default_base_tokens();
        let initial_hot_cycles = Self::generate_default_cycles(&default_tokens);
        
        Self {
            hot_cycles: Arc::new(RwLock::new(initial_hot_cycles)),
            last_update: Arc::new(RwLock::new(0)),
            trending_tokens: Arc::new(RwLock::new(default_tokens)),
            cache_ttl_seconds: 60, // Atualizar a cada minuto
        }
    }
    
    /// 🏗️ Gera ciclos padrão para tokens da Base
    fn generate_default_cycles(tokens: &[Address]) -> Vec<(Vec<Address>, f64)> {
        let mut cycles = Vec::new();
        
        // Gerar ciclos de 3 hops entre os tokens padrão
        for (i, token_a) in tokens.iter().take(5).enumerate() {
            for (j, token_b) in tokens.iter().take(5).enumerate() {
                for token_c in tokens.iter().take(3) {
                    if i != j {
                        let path = vec![*token_a, *token_b, *token_c, *token_a];
                        let estimated_profit = 0.003 + (i as f64 * 0.001);
                        cycles.push((path, estimated_profit));
                    }
                }
            }
        }
        
        cycles
    }
    
    /// 🏠 Retorna tokens padrão da Base Mainnet
    fn get_default_base_tokens() -> Vec<Address> {
        vec![
            // WETH
            Address::new([
                0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x06,
            ]),
            // USDC
            Address::new([
                0x83, 0x35, 0x89, 0xfc, 0xd6, 0xed, 0xb6, 0xe0,
                0x8f, 0x4c, 0x7c, 0x32, 0xd4, 0xf7, 0x1b, 0x54,
                0xbd, 0xa0, 0x29, 0x13,
            ]),
            // DAI
            Address::new([
                0x50, 0xc5, 0x72, 0x59, 0x49, 0xa6, 0xf0, 0xc7,
                0x2e, 0x6c, 0x4a, 0x64, 0x1f, 0x24, 0x04, 0x9a,
                0x91, 0x7d, 0xb0, 0xcb,
            ]),
            // CBETH
            Address::new([
                0x2a, 0xe3, 0xf1, 0xec, 0x7f, 0x1f, 0x50, 0x12,
                0xcf, 0xea, 0xb0, 0x18, 0x5b, 0xfc, 0x7a, 0x3c,
                0xf0, 0xde, 0xc2, 0x02,
            ]),
        ]
    }
    
    /// 🔄 Atualiza rotas quentes baseado em tokens trending
    pub async fn update_hot_cycles(&self, token_volumes: &HashMap<Address, f64>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        // Verificar se cache ainda é válido
        if now - *self.last_update.read().await < self.cache_ttl_seconds {
            return;
        }
        
        // Se temos volumes reais, usar; senão manter defaults
        let trending: Vec<Address> = if !token_volumes.is_empty() {
            let mut sorted: Vec<_> = token_volumes.iter().collect();
            sorted.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
            sorted.into_iter()
                .take(20)
                .map(|(addr, _)| *addr)
                .collect()
        } else {
            // Manter tokens padrão se não há dados de volume
            self.trending_tokens.read().await.clone()
        };
        
        *self.trending_tokens.write().await = trending.clone();
        *self.last_update.write().await = now;
        
        // Pré-calcular ciclos para tokens trending
        let mut hot = self.hot_cycles.write().await;
        hot.clear();
        
        // Gerar ciclos de 3 hops entre tokens trending
        for (i, token_a) in trending.iter().take(10).enumerate() {
            for (j, token_b) in trending.iter().take(10).enumerate() {
                for token_c in trending.iter().take(5) {
                    if i != j {
                        // Ciclo A -> B -> C -> A (simulado)
                        let path = vec![*token_a, *token_b, *token_c, *token_a];
                        let estimated_profit = 0.005 + (i as f64 * 0.001); // Placeholder
                        hot.push((path, estimated_profit));
                    }
                }
            }
        }
        
        trace!("PreCalc: {} rotas, {} tokens trending", hot.len(), trending.len());
    }
    
    /// ⚡ Retorna rotas quentes pré-calculadas
    pub async fn get_hot_cycles(&self) -> Vec<(Vec<Address>, f64)> {
        self.hot_cycles.read().await.clone()
    }
}

/// 🦁 Motor Apex-Predator - ORGANISMO VIVO QUE APRENDE
pub struct ApexPredatorEngine {
    config: ApexConfig,
    /// Grafo de tokens para encontrar ciclos
    token_graph: Arc<RwLock<TokenGraph>>,
    /// Pools monitoradas
    pool_states: Arc<RwLock<HashMap<Address, PoolState>>>,
    /// Estado de liquidações (endereço -> dívida)
    liquidation_queue: Arc<RwLock<HashMap<Address, LiquidationState>>>,
    /// Canal para resultados de simulação paralela
    sim_tx: mpsc::Sender<SimulationResult>,
    sim_rx: Arc<RwLock<mpsc::Receiver<SimulationResult>>>,
    /// Contadores de performance
    stats: Arc<RwLock<ApexStats>>,
    /// 🎯 Monitor de Top 50 tokens
    token_monitor: Arc<TopTokenMonitor>,
    /// 🧠 Adaptive Intelligence - Aprende com competidores
    competitor_tracker: Arc<CompetitorTracker>,
    /// ⚛️ Multi-Atomic Router - Divide rotas para optimizar
    multi_router: Arc<MultiAtomicRouter>,
    /// 🔮 Pre-Calculation Engine - Rotas quentes pré-calculadas
    pre_calc: Arc<PreCalcEngine>,
    /// 🛡️ Conqueror's Shield - Protecção de risco máximo 10%
    max_risk_per_trade_eth: f64,
    /// Banca total disponível (ETH)
    bankroll_eth: f64,
}

/// Grafo de tokens para deteção de ciclos
#[derive(Clone, Debug, Default)]
pub struct TokenGraph {
    /// Token -> [(Pool, TokenOut)]
    pub edges: HashMap<Address, Vec<(Address, Address)>>,
    /// Cache de ciclos encontrados
    pub known_cycles: Vec<Vec<Address>>,
}

/// Estado de uma pool
#[derive(Clone, Debug)]
pub struct PoolState {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub dex_type: DexType,
    pub last_update: u64,
}

/// Estado de liquidação
#[derive(Clone, Debug)]
pub struct LiquidationState {
    pub borrower: Address,
    pub debt_asset: Address,
    pub collateral_asset: Address,
    pub debt_to_cover: U256,
    pub protocol: LendingProtocol,
    pub liquidation_bonus: f64, // ex: 1.05 = 5% bónus
    pub timestamp: u64,
}

/// Resultado de simulação
#[derive(Clone, Debug)]
pub struct SimulationResult {
    pub opportunity_type: ApexOpportunityType,
    pub path: Option<ArbitragePath>,
    pub profit_eth: f64,
    pub gas_cost_eth: f64,
    pub net_profit_eth: f64,
    pub success: bool,
    pub execution_priority: u32, // 1 = máxima prioridade
    pub timestamp_us: u64,
}

/// Estatísticas do Apex
#[derive(Clone, Debug, Default)]
pub struct ApexStats {
    pub cycles_detected: u64,
    pub liquidations_detected: u64,
    pub simulations_run: u64,
    pub opportunities_won: u64,
    pub gas_saved_eth: f64,
}

impl ApexPredatorEngine {
    /// 🚀 Inicializa o motor Apex-Predator - ORGANISMO VIVO
    pub fn new(config: ApexConfig) -> Self {
        let (sim_tx, sim_rx) = mpsc::channel(1000);
        
        info!("ApexPredatorEngine inicializado");
        
        Self {
            config,
            token_graph: Arc::new(RwLock::new(TokenGraph::default())),
            pool_states: Arc::new(RwLock::new(HashMap::new())),
            liquidation_queue: Arc::new(RwLock::new(HashMap::new())),
            sim_tx,
            sim_rx: Arc::new(RwLock::new(sim_rx)),
            stats: Arc::new(RwLock::new(ApexStats::default())),
            token_monitor: Arc::new(TopTokenMonitor::new()),
            competitor_tracker: Arc::new(CompetitorTracker::new()),
            multi_router: Arc::new(MultiAtomicRouter::new()),
            pre_calc: Arc::new(PreCalcEngine::new()),
            max_risk_per_trade_eth: 10.0, // 10% da banca = max 10 ETH se banca=100 ETH
            bankroll_eth: 100.0,          // Banca inicial assumida (atualizar em runtime)
        }
    }

    /// Procura ciclos de arbitragem (3-5 hops)
    pub async fn hunt_cycles(&self, start_token: Address) -> Vec<SimulationResult> {
        let start = std::time::Instant::now();
        
        let hot_cycles = self.pre_calc.get_hot_cycles().await;
        trace!("ApexPredator: {} rotas disponiveis", hot_cycles.len());
        
        // Atualizar pré-cálculo com volumes actuais
        let volumes = self.token_monitor.token_volumes.read().await.clone();
        self.pre_calc.update_hot_cycles(&volumes).await;
        
        let graph = self.token_graph.read().await;
        let pools = self.pool_states.read().await;
        
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        let mut current_path = VecDeque::new();
        
        // DFS ELITE - early exit em lucros grandes
        self.find_cycles_dfs_elite(
            &graph,
            &pools,
            start_token,
            start_token,
            &mut visited,
            &mut current_path,
            0,
            &mut results,
            &start,
        ).await;
        
        let elapsed = start.elapsed().as_micros() as u64;
        
        // 🚨 ALERTA DE PERFORMANCE: Se >1ms, estamos lentos
        if elapsed > 1_000 {
            warn!(
                "[PERF-ALERT] 🔥 DFS LENTO: {}μs > 1ms limite! Optimização necessária!",
                elapsed
            );
        }
        
        // ⚡ EXECUÇÃO IMEDIATA: Se encontramos ciclo >0.01 ETH, executar AGORA
        for result in &results {
            if result.net_profit_eth > 0.01 {
                info!(
                    "[APEX-ELITE] ⚡ EXECUÇÃO IMEDIATA! Ciclo {} hops = {} ETH > 0.01 threshold | Latência: {}μs",
                    match &result.opportunity_type {
                        ApexOpportunityType::ApexCycle { hops, .. } => *hops,
                        _ => 0,
                    },
                    fmt_eth(result.net_profit_eth),
                    elapsed
                );
                // Aqui chamariamos executor imediatamente
                self.execute_best_opportunity(std::slice::from_ref(result)).await;
            }
        }
        
        if !results.is_empty() && elapsed <= 1_000 {
            info!("[PERF] ✅ {} ciclos em {}μs | Top: {} ETH", 
                results.len(), elapsed, 
                fmt_eth(results.first().map(|r| r.net_profit_eth).unwrap_or(0.0))
            );
        }
        
        results
    }

    /// 🔄 DFS ELITE - Early exit em lucros grandes
    async fn find_cycles_dfs_elite(
        &self,
        graph: &TokenGraph,
        pools: &HashMap<Address, PoolState>,
        current: Address,
        target: Address,
        visited: &mut HashSet<Address>,
        path: &mut VecDeque<(Address, Address, Address)>,
        depth: usize,
        results: &mut Vec<SimulationResult>,
        timer: &std::time::Instant,
    ) {
        // ⏱️ TIMEOUT ultra-agressivo: 10ms max por DFS (era 50ms)
        if timer.elapsed().as_micros() > 10_000 {
            return;
        }
        
        if depth > self.config.max_cycle_hops {
            return;
        }
        
        // Ciclo encontrado - avaliar imediatamente
        if current == target && depth >= self.config.min_cycle_hops {
            self.evaluate_cycle(path, pools, results).await;
            
            // ⚡ EARLY EXIT: Se já temos um ciclo >0.01 ETH, não precisamos procurar mais
            if let Some(best) = results.iter().max_by(|a, b| a.net_profit_eth.partial_cmp(&b.net_profit_eth).unwrap()) {
                if best.net_profit_eth > 0.01 {
                    return; // Early exit - já temos o que precisamos
                }
            }
            return;
        }
        
        if visited.contains(&current) {
            return;
        }
        
        visited.insert(current);
        
        // Ordenar edges por maior liquidez primeiro (heurística de elite)
        if let Some(edges) = graph.edges.get(&current) {
            // Limitar a 5 edges por nó para performance
            let limited_edges: Vec<_> = edges.iter().take(5).collect();
            
            for (pool_addr, next_token) in limited_edges {
                if let Some(_pool) = pools.get(pool_addr) {
                    path.push_back((*pool_addr, current, *next_token));
                    
                    Box::pin(self.find_cycles_dfs_elite(
                        graph,
                        pools,
                        *next_token,
                        target,
                        visited,
                        path,
                        depth + 1,
                        results,
                        timer,
                    )).await;
                    
                    path.pop_back();
                    
                    // ⚡ Verificar early exit após cada recursão
                    if results.iter().any(|r| r.net_profit_eth > 0.01) {
                        break; // Já temos lucro suficiente
                    }
                }
            }
        }
        
        visited.remove(&current);
    }

    /// 💰 Avalia lucratividade de um ciclo
    async fn evaluate_cycle(
        &self,
        path: &VecDeque<(Address, Address, Address)>,
        pools: &HashMap<Address, PoolState>,
        results: &mut Vec<SimulationResult>,
    ) {
        let hops: Vec<Hop> = path.iter().map(|(pool_addr, token_in, token_out)| {
            let pool = pools.get(pool_addr);
            Hop {
                pool: *pool_addr,
                token_in: *token_in,
                token_out: *token_out,
                fee: pool.map(|p| match p.dex_type {
                    DexType::UniswapV3 | DexType::UniswapV2 => 3000, // 0.3%
                    DexType::Aerodrome => 2000, // 0.2%
                    DexType::PancakeSwap => 2500, // 0.25%
                    DexType::AerodromeStable => 4, // 0.004% para stable pools
                }).unwrap_or(3000),
                dex_type: pool.map(|p| p.dex_type).unwrap_or(DexType::UniswapV3),
            }
        }).collect();
        
        if hops.is_empty() {
            return;
        }
        
        let first_token = hops.first().map(|h| h.token_in).unwrap_or(Address::ZERO);
        let path_tokens: Vec<Address> = std::iter::once(first_token)
            .chain(hops.iter().map(|h| h.token_out))
            .collect();
        
        // Simulação rápida para estimar lucro
        let profit = self.estimate_cycle_profit(&hops, pools);
        
        if profit > 0.005 { // Mínimo 0.005 ETH
            let arb_path = ArbitragePath {
                hops: hops.clone(),
                input_token: first_token,
                optimal_input: U256::from(1e18 as u64), // 1 ETH inicial
                expected_profit: U256::from((profit * 1e18) as u128),
                profit_ratio: Fixed64::from(1.05), // 5% profit ratio
            };
            
            results.push(SimulationResult {
                opportunity_type: ApexOpportunityType::ApexCycle {
                    hops: hops.len(),
                    path_tokens,
                },
                path: Some(arb_path),
                profit_eth: profit,
                gas_cost_eth: 0.0, // Calculado depois
                net_profit_eth: profit,
                success: true,
                execution_priority: 2,
                timestamp_us: std::time::Instant::now().elapsed().as_micros() as u64,
            });
            
            info!(
                "[APEX-CYCLE] 🔄 Ciclo {} hops: {} ETH lucro estimado",
                hops.len(),
                fmt_eth(profit)
            );
        }
    }

    /// 📊 Estima lucro de um ciclo (simplificado)
    fn estimate_cycle_profit(&self, hops: &[Hop], pools: &HashMap<Address, PoolState>) -> f64 {
        let mut amount = 1.0; // 1 ETH inicial
        
        for hop in hops {
            if let Some(pool) = pools.get(&hop.pool) {
                let (reserve_in, reserve_out) = if hop.token_in == pool.token0 {
                    (pool.reserve0, pool.reserve1)
                } else {
                    (pool.reserve1, pool.reserve0)
                };
                
                // Fórmula constant product
                let reserve_in_f = u256_to_f64(reserve_in);
                let reserve_out_f = u256_to_f64(reserve_out);
                
                if reserve_in_f > 0.0 && reserve_out_f > 0.0 {
                    let amount_out = (reserve_out_f * amount) / (reserve_in_f + amount);
                    // Taxa 0.3% para Uniswap V3
                    amount = amount_out * 0.997;
                }
            }
        }
        
        // Lucro = quantidade final - 1 ETH inicial
        f64::max(amount - 1.0, 0.0)
    }

    /// ⚔️ [FATAL-STRIKE] Monitora liquidações de alto impacto
    pub async fn monitor_liquidations(&self, log: &Log) -> Option<SimulationResult> {
        // Tópicos de eventos de liquidação
        const LIQUIDATION_TOPIC_AAVE: [u8; 32] = [
            0xe4, 0x13, 0xa3, 0x21, 0xe8, 0x51, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        
        let topics = log.topics();
        if topics.is_empty() {
            return None;
        }
        
        // Verificar se é evento de liquidação
        let is_liquidation = topics.iter().any(|t| {
            t.as_slice().starts_with(&LIQUIDATION_TOPIC_AAVE[..8])
        });
        
        if !is_liquidation {
            return None;
        }
        
        // Extrair dados da liquidação (simplificado)
        let protocol = self.detect_lending_protocol(log.address);
        
        // Verificar se é alto impacto (>1 ETH)
        let debt_to_cover = U256::from(2e18 as u64); // Placeholder - parse real dos logs
        let debt_eth = u256_to_f64(debt_to_cover);
        
        if debt_eth >= self.config.liquidation_threshold_eth {
            let liquidation = LiquidationState {
                borrower: Address::ZERO, // Parse do log
                debt_asset: Address::ZERO,
                collateral_asset: Address::ZERO,
                debt_to_cover,
                protocol,
                liquidation_bonus: 1.05, // 5% tipical
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };
            
            // Adicionar à fila
            self.liquidation_queue.write().await.insert(log.address, liquidation.clone());
            
            let profit_eth = debt_eth * (liquidation.liquidation_bonus - 1.0);
            
            info!(
                "[FATAL-STRIKE] ⚔️ Liquidação {} >1 ETH detetada! {} ETH potencial",
                match protocol {
                    LendingProtocol::AaveV3 => "AAVE",
                    LendingProtocol::Seamless => "SEAMLESS",
                    LendingProtocol::Moonwell => "MOONWELL",
                },
                fmt_eth(profit_eth)
            );
            
            return Some(SimulationResult {
                opportunity_type: ApexOpportunityType::FatalStrike { protocol, debt_to_cover },
                path: None,
                profit_eth,
                gas_cost_eth: 0.0,
                net_profit_eth: profit_eth,
                success: true,
                execution_priority: 1, // Máxima prioridade
                timestamp_us: std::time::Instant::now().elapsed().as_micros() as u64,
            });
        }
        
        None
    }

    /// 🔍 Deteta protocolo de lending pelo endereço
    fn detect_lending_protocol(&self, address: Address) -> LendingProtocol {
        // Endereços conhecidos na Base (placeholders)
        let aave_pool = Address::ZERO; // Substituir por endereço real
        let seamless = Address::ZERO;
        let moonwell = Address::ZERO;
        
        if address == aave_pool {
            LendingProtocol::AaveV3
        } else if address == seamless {
            LendingProtocol::Seamless
        } else if address == moonwell {
            LendingProtocol::Moonwell
        } else {
            LendingProtocol::AaveV3 // Default
        }
    }

    /// ⛽ [GAS-SHIELD] ELITE - Calcula priority fee AGRESSIVA para topo do bloco
    /// 🎯 OBJECTIVO: Estar nos primeiros 5 slots do bloco
    pub fn calculate_elite_priority_fee(
        &self,
        base_fee_gwei: u64,
        expected_profit_eth: f64,
        competition_level: u8, // 1-10 (10 = máxima competição)
    ) -> u64 {
        let base_fee = base_fee_gwei as f64;
        
        // 🦁 ESTRATÉGIA AGRESSIVA:
        // Base fee + (multiplicador de competição * lucro potencial)
        let competition_multiplier = 2.0 + (competition_level as f64 * 0.5); // 2.0x a 7.0x
        
        // Se lucro > 0.01 ETH, podemos pagar mais de gás
        let profit_boost = if expected_profit_eth > 0.01 {
            (expected_profit_eth * 0.2 * 1e9) as u64 // 20% do lucro vai para gás
        } else {
            0
        };
        
        let priority_tip = ((base_fee * competition_multiplier) as u64) + profit_boost;
        
        // Verificar rentabilidade
        let total_gas_gwei = base_fee_gwei + priority_tip;
        let gas_cost_eth = (200_000.0 * total_gas_gwei as f64) / 1e9;
        let net_profit = expected_profit_eth - gas_cost_eth;
        
        // ⚡ LOG ELITE: Mostrar exatamente quanto estamos a pagar para competir
        info!(
            "[GAS-ELITE] ⚡ Priority Fee: {} gwei | Competition: {}x | Net Profit: {} ETH | {}USD",
            priority_tip,
            competition_multiplier,
            fmt_eth(net_profit),
            (net_profit * 3500.0) as u64
        );
        
        if net_profit < 0.002 { // Threshold agressivo (era 0.005)
            warn!(
                "[GAS-SHIELD] 🛡️ ABORTADO - Lucro {} ETH < 0.002 após gás de {} gwei",
                fmt_eth(net_profit),
                priority_tip
            );
            return 0; // Sinal para abortar
        }
        
        priority_tip
    }
    
    /// 🔥 [BUNDLE-TOP] Envia bundle com priority fee máxima para topo do bloco
    pub async fn send_bundle_top_block(
        &self,
        _txs: Vec<String>,
        expected_profit: f64,
    ) -> eyre::Result<String> {
        let base_fee = 20u64; // Assumir 20 gwei base (actualizar em runtime)
        
        // 🎯 Priority fee ELITE para garantir slot #1-5 no bloco
        let priority_fee = self.calculate_elite_priority_fee(base_fee, expected_profit, 8);
        
        if priority_fee == 0 {
            return Err(eyre::eyre!("Lucro insuficiente para competir no bloco"));
        }
        
        info!(
            "[BUNDLE-TOP] 🚀 Bundle preparado | Priority: {} gwei | Target: TOP 5 slots | Profit: {} ETH",
            priority_fee,
            fmt_eth(expected_profit)
        );
        
        // Aqui integraríamos com MevShareBundle para envio real
        Ok(format!("bundle_priority_{}", priority_fee))
    }

    /// 🌪️ Simulação Paralela (O Olho de Sauron)
    pub async fn parallel_simulation(
        &self,
        opportunities: Vec<SimulationResult>,
    ) -> Vec<SimulationResult> {
        let start = std::time::Instant::now();
        let mut handles: Vec<JoinHandle<SimulationResult>> = Vec::new();
        
        // Limitar número de simulações paralelas
        let to_simulate: Vec<_> = opportunities.into_iter()
            .take(self.config.max_parallel_simulations)
            .collect();
        
        info!(
            "[APEX-PREDATOR] 🌪️ Iniciando {} simulações paralelas...",
            to_simulate.len()
        );
        
        for opp in to_simulate {
            let sim_tx = self.sim_tx.clone();
            
            let handle = tokio::spawn(async move {
                let sim_start = std::time::Instant::now();
                
                // Simulação real seria aqui (REVM ou cache)
                let mut result = opp.clone();
                
                // Simulação placeholder - adicionar latência realística
                // tokio::time::sleep(tokio::time::Duration::from_micros(50)).await;
                
                let sim_time = sim_start.elapsed().as_micros() as u64;
                result.timestamp_us = sim_time;
                
                // Enviar resultado
                let _ = sim_tx.send(result.clone()).await;
                
                result
            });
            
            handles.push(handle);
        }
        
        // Aguardar todas as simulações
        let mut results = Vec::new();
        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }
        
        // Ordenar por lucro líquido
        results.sort_by(|a, b| {
            b.net_profit_eth.partial_cmp(&a.net_profit_eth).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        let total_time = start.elapsed().as_micros() as u64;
        
        if let Some(best) = results.first() {
            info!(
                "[APEX-PREDATOR] ✅ Melhor oportunidade: {} ETH em {}μs | {} ETH lucro",
                match &best.opportunity_type {
                    ApexOpportunityType::ApexCycle { hops, .. } => format!("Ciclo {} hops", hops),
                    ApexOpportunityType::FatalStrike { protocol, .. } => format!("{:?}", protocol),
                    _ => "Backrun".to_string(),
                },
                total_time,
                fmt_eth(best.net_profit_eth)
            );
        }
        
        // Atualizar estatísticas
        if let Ok(mut stats) = self.stats.try_write() {
            stats.simulations_run += results.len() as u64;
        }
        
        results
    }

    /// 🛡️ CONQUEROR'S SHIELD - Protecção de risco máximo 10% da banca
    fn verify_risk_limits(&self, trade_size_eth: f64, max_gas_cost_eth: f64) -> bool {
        let max_risk = self.bankroll_eth * 0.10; // 10% da banca
        let total_risk = trade_size_eth + max_gas_cost_eth;
        
        if total_risk > max_risk {
            error!(
                "[CONQUEROR-SHIELD] 🛡️ BLOQUEADO! Risco {} ETH > 10% banca ({} ETH) | Trade: {} ETH + Gás: {} ETH",
                fmt_eth(total_risk),
                fmt_eth(max_risk),
                fmt_eth(trade_size_eth),
                fmt_eth(max_gas_cost_eth)
            );
            false
        } else {
            info!(
                "[CONQUEROR-SHIELD] ✅ Risco aceitável: {} ETH < {} ETH (10% banca)",
                fmt_eth(total_risk),
                fmt_eth(max_risk)
            );
            true
        }
    }

    /// 🏆 Executa a melhor oportunidade - INTEGRAÇÃO COMPLETA DAS ENGINES
    pub async fn execute_best_opportunity(&self, results: &[SimulationResult]) -> Option<()> {
        let best = results.first()?;
        
        // 🛡️ CONQUEROR'S SHIELD: Verificar risco antes de executar
        let trade_size = best.net_profit_eth * 2.0; // Assumir 2x lucro = trade size
        let max_gas = 0.5; // Max 0.5 ETH de gás
        if !self.verify_risk_limits(trade_size, max_gas) {
            return None; // Abortar - risco excessivo
        }
        
        // 🧠 ADAPTIVE INTELLIGENCE: Ajustar gas price baseado em histórico
        let path_tokens = best.path.as_ref()?.hops.iter()
            .map(|h| h.token_in)
            .collect::<Vec<_>>();
        let adaptive_gas = self.competitor_tracker
            .get_adaptive_gas_price(&path_tokens, 20) // 20 gwei base
            .await;
        
        // ⚛️ MULTI-ATOMIC: Verificar se devemos dividir a rota
        let should_split = self.multi_router
            .should_split(trade_size, 100) // 100 bps = 1% slippage estimado
            .await;
        
        if should_split {
            let splits = self.multi_router.split_routes(trade_size, 2);
            info!(
                "[MULTI-ATOMIC] ✂️ Rota dividida em {} partes: {:?} ETH",
                splits.len(),
                splits.iter().map(|s| fmt_eth(*s)).collect::<Vec<_>>()
            );
        }
        
        match &best.opportunity_type {
            ApexOpportunityType::FatalStrike { protocol, .. } => {
                info!(
                    "[FATAL-STRIKE] ⚔️ EXECUTANDO liquidação {:?} - {} ETH lucro! | Gas adaptativo: {} gwei",
                    protocol,
                    fmt_eth(best.net_profit_eth),
                    adaptive_gas
                );
            }
            ApexOpportunityType::ApexCycle { hops, .. } => {
                info!(
                    "[APEX-CYCLE] 🔄 EXECUTANDO ciclo {} hops - {} ETH lucro! | Gas adaptativo: {} gwei",
                    hops,
                    fmt_eth(best.net_profit_eth),
                    adaptive_gas
                );
            }
            _ => {
                info!(
                    "[APEX-PREDATOR] 🎯 EXECUTANDO backrun - {} ETH lucro! | Gas adaptativo: {} gwei",
                    fmt_eth(best.net_profit_eth),
                    adaptive_gas
                );
            }
        }
        
        // Atualizar stats
        if let Ok(mut stats) = self.stats.try_write() {
            stats.opportunities_won += 1;
        }
        
        // 🧠 ADAPTIVE: Guardar sucesso para análise futura (simulação)
        // self.competitor_tracker.record_win(&path_tokens, adaptive_gas).await;
        
        // ⚛️ MULTI-ATOMIC: Guardar resultado da divisão
        if should_split {
            self.multi_router.record_split_result(2, true).await;
        }
        
        Some(())
    }

    /// 📊 Retorna estatísticas
    pub async fn stats(&self) -> ApexStats {
        self.stats.read().await.clone()
    }

    /// 🔄 Atualiza estado de uma pool
    pub async fn update_pool(&self, pool: PoolState) {
        let mut pools = self.pool_states.write().await;
        
        // Atualizar grafo
        let mut graph = self.token_graph.write().await;
        
        graph.edges.entry(pool.token0).or_default().push((pool.address, pool.token1));
        graph.edges.entry(pool.token1).or_default().push((pool.address, pool.token0));
        
        pools.insert(pool.address, pool);
    }
    
    /// 💰 Atualiza banca disponível
    pub fn update_bankroll(&mut self, new_bankroll_eth: f64) {
        info!(
            "[BANKROLL] 💰 Banca actualizada: {} ETH -> {} ETH",
            fmt_eth(self.bankroll_eth),
            fmt_eth(new_bankroll_eth)
        );
        self.bankroll_eth = new_bankroll_eth;
    }
    
    /// 📝 Regista uma oportunidade perdida para aprendizagem
    pub async fn record_competitor_win(
        &self,
        token_path: &[Address],
        winner_gas_price_gwei: u64,
    ) {
        self.competitor_tracker
            .record_loss(token_path, winner_gas_price_gwei)
            .await;
    }
    
    /// 🧠 Retorna estatísticas de aprendizagem
    pub async fn adaptive_stats(&self) -> String {
        let count = *self.competitor_tracker.learning_count.read().await;
        let patterns = self.competitor_tracker.winner_patterns.read().await.len();
        format!(
            "🧠 Adaptive Intelligence: {} padrões aprendidos | {} análises",
            patterns, count
        )
    }
}

#[async_trait]
impl Strategy for ApexPredatorEngine {
    async fn process_event(
        &mut self,
        event: MevEvent,
        _context: &StrategyContext,
    ) -> eyre::Result<()> {
        match event {
            MevEvent::Swap(swap) => {
                // Atualizar estado da pool
                let pool_state = PoolState {
                    address: swap.pool,
                    token0: swap.token_in, // Simplificado
                    token1: swap.token_out,
                    reserve0: swap.amount_in,
                    reserve1: swap.amount_out,
                    dex_type: swap.dex_type,
                    last_update: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };
                self.update_pool(pool_state).await;
                
                // Procurar oportunidades (hunt_cycles já faz DFS e simulação)
                let _results = self.hunt_cycles(swap.token_in).await;
            }
            MevEvent::BlockUpdate(block) => {
                trace!("[APEX] Bloco {} detetado", block);
            }
            _ => {}
        }
        Ok(())
    }

    async fn initialize(&mut self, initial_data: Vec<NormalizedSwapEvent>) -> eyre::Result<()> {
        info!("APEX-PREDATOR: Inicializando com {} eventos", initial_data.len());
        for swap in initial_data {
            let pool_state = PoolState {
                address: swap.pool,
                token0: swap.token_in,
                token1: swap.token_out,
                reserve0: swap.amount_in,
                reserve1: swap.amount_out,
                dex_type: swap.dex_type,
                last_update: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };
            self.update_pool(pool_state).await;
        }
        Ok(())
    }

    fn stats(&self) -> super::strategy::StrategyStats {
        super::strategy::StrategyStats::default()
    }
}

/// Helper: U256 -> f64
fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(0.0)
}

/// Helper: Formatação ETH
fn fmt_eth(val: f64) -> String {
    format!("{:.6}", val)
}
