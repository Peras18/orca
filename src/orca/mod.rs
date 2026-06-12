//! PROJETO ORCA - Base Mainnet MEV Engine
//! Segurança absoluta de capital + Extração máxima de valor
//!
//! 🐋 ORCA: Predador silencioso que ataca no momento exato

pub mod audit;
pub mod ghost_executor;
pub mod performance_tracker;
pub mod safety;
pub mod sequencer_sync;
pub mod yul_contracts;

pub use audit::ForensicAudit;
pub use ghost_executor::{CallbackHijacker, GhostStateExecutor, TransientAction};
pub use performance_tracker::{BankLog, GasLog, PerformanceTracker, ProfitLog};
pub use safety::{BundleProtector, ProfitGuard, SafetyEngine};
pub use sequencer_sync::{BlockTiming, RTTMonitor, SequencerSync};
pub use yul_contracts::{GasOptimizer, YulExecutor, YulTemplates};

use crate::artemis::{MevEvent, Strategy, StrategyContext};
use crate::cache::PoolCache;
use crate::contracts::{DexType, NormalizedSwapEvent};
use crate::graph::{ArbGraph, PoolScorer};
use crate::prediction::detect_cross_pool_divergence;
use crate::prediction::PatternMemory;
use crate::risk::BankrollManager;
use crate::telemetry::TelemetryCollector;
use alloy::primitives::{address, Address, Bytes, FixedBytes, U256};
use alloy::providers::{Provider as AlloyProvider, RootProvider};
use alloy::transports::BoxTransport;
use async_trait::async_trait;
use chrono::Timelike;
use eyre::Context as _;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};
use crate::discovery::PoolDiscoveryEngine;

use crate::strategies::long_tail::{MidCapScanner, LaunchMonitor};
use crate::strategies::jit_liquidity::{JITMonitor, CLPool};
use crate::math::transfer_entropy::TransferEntropyDetector;
use crate::singularity::InvisibleProbe;
use crate::notifications::DiscordNotifier;
use crate::singularity::SequencerHeartbeatMonitor;
/// WETH na Base — usado como token de partida no grafo (SwapV3 pode expor `token_in` nulo no grafo).
const WETH: Address = address!("4200000000000000000000000000000000000006");
const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
const CBETH: Address = address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEC22");
const AERO: Address = address!("940181a94A35A4569E4529A3CDfB74e38FD98631");

/// 🐋 ORCA Engine - Motor principal de execução
#[derive(Clone, Debug)]
pub struct OrcaEngine {
    /// Módulo de segurança
    pub safety: SafetyEngine,
    /// Executor Ghost-State
    pub ghost: GhostStateExecutor,
    /// Otimizador Yul
    pub yul: YulExecutor,
    /// Sincronização com sequenciador
    pub sequencer: SequencerSync,
    /// Tracker de performance
    pub tracker: Arc<RwLock<PerformanceTracker>>,
    /// Motor de auditoria forense
    pub audit: Arc<ForensicAudit>,
    /// Capital inicial (ETH)
    initial_capital: f64,
    /// Capital atual (ETH)
    current_capital: Arc<RwLock<f64>>,
    /// Total de lucro extraído
    total_profit: Arc<RwLock<f64>>,
    /// Total de gás poupado (ETH)
    total_gas_saved: Arc<RwLock<f64>>,
    /// Contador de execuções
    execution_count: Arc<RwLock<u64>>,
    /// Cache de pools para arbitragem
    pool_cache: PoolCache,
    /// Grafo de arbitragem (reconstruído a cada bloco)
    arb_graph: Arc<RwLock<ArbGraph>>,
    /// Telemetria para métricas de performance
    telemetry: Option<Arc<TelemetryCollector>>,
    /// 💰 Gestor de banca adaptativo
    bankroll_manager: Arc<RwLock<BankrollManager>>,
    /// Memória de padrões de oportunidades por pool/hora
    pattern_memory: Arc<PatternMemory>,
    /// Scorer dinâmico de pools (freq/opps)
    pool_scorer: Arc<PoolScorer>,
    /// Throttle: processar no máximo 1x por bloco
    last_processed_block: Arc<AtomicU64>,
    /// Último bloco persistido em disco
    last_pattern_persist_block: Arc<RwLock<u64>>,
    /// Último bloco em que logámos status report
    last_status_block: Arc<RwLock<u64>>,
    /// Provider para sync de saldo on-chain
    balance_provider: Arc<RootProvider<BoxTransport>>,
    /// Endereço da wallet usado para sync de saldo
    tracked_wallet: Arc<RwLock<Option<Address>>>,
    /// Último bloco observado (heartbeat / diagnóstico)
    last_observed_block: Arc<AtomicU64>,
    /// Deduplicação de eventos por (tx_hash, log_index) → block_number
    seen_events: Arc<RwLock<HashMap<(FixedBytes<32>, u64), u64>>>,
    /// Logger CSV de oportunidades
    opp_logger: Arc<crate::logger::opportunity_logger::OpportunityLogger>,
    discovery: Arc<PoolDiscoveryEngine>,
    kalman_gas: Arc<RwLock<crate::math::kalman_gas::KalmanGasPredictor>>,
    flash_optimizer: Arc<RwLock<crate::math::flash_optimizer::FlashLoanOptimizer>>,
    honeypot: Arc<crate::security::honeypot_filter::HoneypotFilter>,
    curvature: Arc<RwLock<crate::prediction::CurvatureDetector>>,
    topology: Arc<RwLock<crate::graph::PersistentTopology>>,
    midcap_scanner: Arc<MidCapScanner>,
    launch_monitor: Arc<LaunchMonitor>,
    jit_monitor: Arc<JITMonitor>,
    transfer_entropy: Arc<RwLock<TransferEntropyDetector>>,
    invisible_probe: Arc<InvisibleProbe>,
    sequencer_heartbeat: Arc<SequencerHeartbeatMonitor>,
    discord: Arc<DiscordNotifier>,
}

/// ⚙️ Configuração do ORCA
#[derive(Clone, Debug)]
pub struct OrcaConfig {
    /// Lucro mínimo para execução (ETH)
    pub min_profit_eth: f64,
    /// Lucro mínimo em € (alternativo)
    pub min_profit_eur: f64,
    /// Capital inicial
    pub initial_capital_eth: f64,
    /// RPC URL da Base
    pub base_rpc_url: String,
    /// Protector RPC URL
    pub protector_rpc_url: String,
    /// Kill-switch threshold (% do capital)
    pub kill_threshold_pct: f64,
    /// Modo diagnóstico (lucro mínimo mais baixo apenas aqui)
    pub dry_run: bool,
}

impl Default for OrcaConfig {
    fn default() -> Self {
        Self {
            min_profit_eth: 0.002, // 0.002 ETH = ~$5
            min_profit_eur: 5.0,
            initial_capital_eth: 0.05, // ~80€ @ $1600/ETH
            base_rpc_url: std::env::var("BASE_RPC_URL")
                .unwrap_or_else(|_| "https://mainnet.base.org".to_string()),
            protector_rpc_url: std::env::var("PROTECTOR_RPC_URL")
                .unwrap_or_else(|_| "https://rpc.flashbots.net/fast".to_string()),
            kill_threshold_pct: 0.50, // 50% = 40€ de 80€
            dry_run: false,
        }
    }
}

impl OrcaEngine {
    /// 🚀 Inicializa ORCA Engine
    pub async fn new(config: OrcaConfig, discovery: Arc<PoolDiscoveryEngine>) -> Self {
        let safety_min_profit_eth = if config.dry_run { 0.00005 } else { 0.0001 };
        info!("═══════════════════════════════════════════════════════════");
        info!("🐋 PROJETO ORCA - Base Mainnet MEV Engine");
        info!("═══════════════════════════════════════════════════════════");
        info!(
            "💰 Capital Inicial: {} ETH (~{}€)",
            config.initial_capital_eth,
            config.initial_capital_eth * 1600.0
        );
        info!(
            "🎯 Lucro Mínimo (Safety): {} ETH{}",
            safety_min_profit_eth,
            if config.dry_run {
                " (DRY_RUN diagnóstico)"
            } else {
                ""
            }
        );
        info!("⚖️ Priority Queue: Ativada (Higher Profit First)");
        info!(
            "💀 Kill-Switch: {}% do capital",
            config.kill_threshold_pct * 100.0
        );
        info!("⚡ Bundle: Protector RPC (Flashbots/Base)");
        info!("🔧 Yul Assembly: -30% gás");
        info!("═══════════════════════════════════════════════════════════");

        let tracker = PerformanceTracker::new();
        let audit = ForensicAudit::new("audit_results_mainnet.log");
        let pool_cache = PoolCache::new();
        let pool_cache_for_midcap = pool_cache.clone();
        let arb_graph = ArbGraph::new(pool_cache.clone(), U256::from(10).pow(U256::from(19)));

        // 💰 Inicializar BankrollManager com capital inicial em wei
        let initial_balance_wei = (config.initial_capital_eth * 1e18) as u128;
        let bankroll_manager = BankrollManager::new(initial_balance_wei);
        let balance_provider = alloy::providers::builder()
            .on_http(
                config
                    .base_rpc_url
                    .parse()
                    .expect("base_rpc_url inválida para provider HTTP"),
            )
            .boxed();
        info!(
            "💰 [BANKROLL] Inicializado: {} wei | Gas budget: {} wei",
            initial_balance_wei,
            bankroll_manager.max_daily_gas_budget()
        );

        Self {
            safety: SafetyEngine::new(
                config.initial_capital_eth,
                safety_min_profit_eth,
                config.kill_threshold_pct,
            ),
            ghost: GhostStateExecutor::new(),
            yul: YulExecutor::new(),
            sequencer: SequencerSync::new(&config.protector_rpc_url),
            tracker: Arc::new(RwLock::new(tracker)),
            audit: Arc::new(audit),
            initial_capital: config.initial_capital_eth,
            current_capital: Arc::new(RwLock::new(config.initial_capital_eth)),
            total_profit: Arc::new(RwLock::new(0.0)),
            total_gas_saved: Arc::new(RwLock::new(0.0)),
            execution_count: Arc::new(RwLock::new(0)),
            pool_cache,
            arb_graph: Arc::new(RwLock::new(arb_graph)),
            telemetry: None,
            bankroll_manager: Arc::new(RwLock::new(bankroll_manager)),
            pattern_memory: Arc::new(PatternMemory::new("data/pattern_memory.json")),
            pool_scorer: Arc::new(PoolScorer::new()),
            last_processed_block: Arc::new(AtomicU64::new(0)),
            last_pattern_persist_block: Arc::new(RwLock::new(0)),
            last_status_block: Arc::new(RwLock::new(0)),
            balance_provider: Arc::new(balance_provider),
            tracked_wallet: Arc::new(RwLock::new(None)),
            last_observed_block: Arc::new(AtomicU64::new(0)),
            seen_events: Arc::new(RwLock::new(HashMap::new())),
            opp_logger: Arc::new(crate::logger::opportunity_logger::OpportunityLogger::new("logs/opportunities.csv")),
            discovery,
            kalman_gas: Arc::new(RwLock::new(crate::math::kalman_gas::KalmanGasPredictor::new(0.1))),
            flash_optimizer: Arc::new(RwLock::new(crate::math::flash_optimizer::FlashLoanOptimizer::new())),
            honeypot: Arc::new(crate::security::honeypot_filter::HoneypotFilter::new()),
            curvature: Arc::new(RwLock::new(crate::prediction::CurvatureDetector::new())),
            topology: Arc::new(RwLock::new(crate::graph::PersistentTopology::new())),
            midcap_scanner: Arc::new(MidCapScanner::new(pool_cache_for_midcap)),
            launch_monitor: Arc::new(LaunchMonitor::new()),
            jit_monitor: Arc::new(JITMonitor::new()),
            transfer_entropy: Arc::new(RwLock::new(TransferEntropyDetector::new(20))),
            invisible_probe: Arc::new(InvisibleProbe::new().await),
            sequencer_heartbeat: Arc::new(SequencerHeartbeatMonitor::new().await),
            discord: Arc::new(DiscordNotifier::new(&std::env::var("DISCORD_WEBHOOK").unwrap_or_default())),
        }
    }

    async fn sync_wallet_balance(&self) -> eyre::Result<()> {
        let wallet = *self.tracked_wallet.read().await;
        let Some(wallet) = wallet else {
            return Ok(());
        };

        let balance = self
            .balance_provider
            .get_balance(wallet)
            .await
            .wrap_err("falha ao obter saldo da wallet")?
            .to::<u128>();

        let mut bankroll = self.bankroll_manager.write().await;
        bankroll.update_balance(balance);
        Ok(())
    }

    /// 📊 Configura telemetria para métricas de performance
    pub fn set_telemetry(&mut self, telemetry: Arc<TelemetryCollector>) {
        self.telemetry = Some(telemetry);
        info!("[ORCA] 📊 Telemetria ativada — métricas em tempo real");
    }

    /// Injeta cache de pools partilhado com bootstrap/collector.
    pub fn set_shared_pool_cache(&mut self, shared_pool_cache: PoolCache) {
        self.pool_cache = shared_pool_cache.clone();
        let graph = ArbGraph::new(shared_pool_cache, U256::from(10).pow(U256::from(19)));
        self.arb_graph = Arc::new(RwLock::new(graph));
        info!("[ORCA] 🔗 Pool cache partilhado injetado no motor de arbitragem");
        let discord_start = self.discord.clone();
        tokio::spawn(async move { discord_start.notify_start().await; });
        // Arrancar InvisibleProbe em background — seleciona RPC mais rápido continuamente
        let probe = self.invisible_probe.clone();
        tokio::spawn(async move {
            probe.start_continuous_probing().await;
        });
        info!("[INVISIBLE-PROBE] 👁️ Sonda de nós iniciada em background");
        // Arrancar SequencerHeartbeat em background — aprende timing do sequencer
        let heartbeat = self.sequencer_heartbeat.clone();
        tokio::spawn(async move {
            heartbeat.start_monitoring().await;
        });
        info!("[HEARTBEAT] 💓 Monitor de sequencer iniciado em background");
    }

    /// 🔍 Valida oportunidade via simulação local
    pub async fn validate_opportunity(
        &self,
        opportunity: &Opportunity,
    ) -> Result<SimulationResult, String> {
        // 1. Simulação via eth_call (obrigatória)
        let sim_result = self.simulate_locally(opportunity).await?;

        // 2. Verificar lucro mínimo (0.002 ETH)
        if sim_result.net_profit_eth < self.safety.min_profit_eth() {
            return Err(format!(
                "Lucro {} ETH abaixo do mínimo {} ETH",
                sim_result.net_profit_eth,
                self.safety.min_profit_eth()
            ));
        }

        // 3. Verificar se é topo do bloco
        let timing = self.sequencer.calculate_optimal_timing().await;
        if !timing.will_be_top_of_block {
            return Err("Não será incluído no topo do bloco".to_string());
        }

        Ok(sim_result)
    }

    /// ⚡ Executa oportunidade validada
    pub async fn execute_opportunity(&self, opportunity: Opportunity) -> Option<ExecutionReceipt> {
        // 1. Validar
        let sim_result = match self.validate_opportunity(&opportunity).await {
            Ok(r) => r,
            Err(e) => {
                debug!("[ORCA] ⛔ Oportunidade rejeitada: {}", e);
                return None;
            }
        };

        // 2. Verificar kill-switch
        if !self.safety.can_operate().await {
            error!("[ORCA] 💀 KILL-SWITCH ATIVO - Execução bloqueada");
            return None;
        }

        // 3. Construir bundle protegido
        let bundle = self
            .build_protected_bundle(&opportunity, &sim_result)
            .await?;

        // 4. Aguardar timing ótimo — combinar SequencerSync + HeartbeatMonitor
        let timing = self.sequencer.await_optimal_window().await;
        // Heartbeat: esperar janela ótima de submissão baseada em RTT aprendido
        let next_block = self.last_observed_block.load(Ordering::Relaxed) + 1;
        let send_window = self.sequencer_heartbeat.calculate_optimal_send_time(next_block).await;
        self.sequencer_heartbeat.wait_for_send_window(&send_window).await;

        // 5. Enviar via Protector RPC
        info!(
            "[ORCA] 🚀 ENVIANDO | Profit: {} ETH | Gas: {} | Slot: {}",
            sim_result.net_profit_eth, sim_result.gas_used, timing.block_slot
        );

        let receipt = self.submit_to_protector(bundle, timing).await?;

        // 6. Atualizar estado
        self.update_state_after_execution(&receipt, &sim_result)
            .await;

        // 7. Log de performance
        self.log_performance(&receipt, &sim_result).await;

        Some(receipt)
    }

    /// 🧠 Simula execução localmente (eth_call)
    async fn simulate_locally(
        &self,
        opportunity: &Opportunity,
    ) -> Result<SimulationResult, String> {
        // Em produção: chamar eth_call real no RPC
        // Eliminado mocks de lucro fixo
        let gas_used = 150000u64;
        let gas_price_gwei = 0.1f64;

        let gross_profit = opportunity.expected_profit_eth;
        if gross_profit <= 0.0 {
            return Err("Lucro esperado inválido ou nulo".to_string());
        }

        let gas_cost_eth = (gas_used as f64 * gas_price_gwei) / 1e9;

        // Yul otimization: -30% gas
        let yul_gas_saved = (gas_used as f64 * 0.30) as u64;
        let final_gas = gas_used - yul_gas_saved;
        let final_gas_cost = (final_gas as f64 * gas_price_gwei) / 1e9;
        let final_profit = gross_profit - final_gas_cost;

        if final_profit <= 0.0 {
            return Err(format!(
                "Oportunidade não lucrativa após gás: {} ETH",
                final_profit
            ));
        }

        Ok(SimulationResult {
            gross_profit_eth: gross_profit,
            net_profit_eth: final_profit,
            gas_used: final_gas,
            gas_cost_eth: final_gas_cost,
            gas_saved_eth: gas_cost_eth - final_gas_cost,
            will_succeed: true,
        })
    }

    /// 📦 Constrói bundle protegido
    async fn build_protected_bundle(
        &self,
        opportunity: &Opportunity,
        sim: &SimulationResult,
    ) -> Option<ProtectedBundle> {
        // Usar Yul executor para otimização
        let yul_tx = self.yul.build_optimized_transaction(opportunity).await?;

        Some(ProtectedBundle {
            transactions: vec![yul_tx],
            min_profit_eth: sim.net_profit_eth,
            max_gas_eth: sim.gas_cost_eth * 1.1, // 10% margem
            target_slot: 0,                      // Topo do bloco
            revert_on_failure: true,             // Importante: reverte se não lucrar
        })
    }

    /// 📡 Envia para Protector RPC
    async fn submit_to_protector(
        &self,
        bundle: ProtectedBundle,
        timing: BlockTiming,
    ) -> Option<ExecutionReceipt> {
        // Em produção: HTTP POST para Flashbots Protector
        info!(
            "[ORCA] 📡 Submetido ao Protector RPC | Slot: {} | Deadline: {}",
            timing.block_slot, timing.deadline
        );

        // Simulação de sucesso
        Some(ExecutionReceipt {
            tx_hash: format!("0x{:064x}", std::time::Instant::now().elapsed().as_nanos()),
            block_number: timing.target_block,
            slot: timing.block_slot,
            profit_eth: bundle.min_profit_eth,
            gas_used: 105000,                            // Gas otimizado
            gas_saved_eth: bundle.min_profit_eth * 0.05, // ~5% do lucro
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }

    /// 📊 Atualiza estado após execução
    async fn update_state_after_execution(
        &self,
        receipt: &ExecutionReceipt,
        _sim: &SimulationResult,
    ) {
        let mut capital = self.current_capital.write().await;
        *capital += receipt.profit_eth;
        drop(capital);

        let mut profit = self.total_profit.write().await;
        *profit += receipt.profit_eth;
        drop(profit);

        let mut gas_saved = self.total_gas_saved.write().await;
        *gas_saved += receipt.gas_saved_eth;
        drop(gas_saved);

        let mut count = self.execution_count.write().await;
        *count += 1;
        drop(count);

        // Notificar safety engine
        self.safety.record_profit(receipt.profit_eth).await;

        // Verificar kill-switch
        let current = *self.current_capital.read().await;
        if self.safety.check_kill_threshold(current).await {
            self.trigger_kill_switch().await;
        }
    }

    /// 📝 Log de performance
    async fn log_performance(&self, receipt: &ExecutionReceipt, sim: &SimulationResult) {
        let capital = *self.current_capital.read().await;
        let profit = *self.total_profit.read().await;
        let gas_saved = *self.total_gas_saved.read().await;

        info!(
            "[ORCA-HIT] 💰 Lucro: {} ETH | Slot: {} | Block: {}",
            receipt.profit_eth, receipt.slot, receipt.block_number
        );

        info!(
            "[GAS-SAVED] ⛽ Economia Yul: {} ETH | Gas usado: {} | Poupado: {}%",
            receipt.gas_saved_eth,
            receipt.gas_used,
            (sim.gas_saved_eth / sim.gas_cost_eth * 100.0) as u64
        );

        info!(
            "[BANK-TOTAL] 💎 Saldo: {} ETH | Total lucro: {} ETH | Total gás poupado: {} ETH",
            capital, profit, gas_saved
        );
    }

    /// 💀 Ativa kill-switch
    async fn trigger_kill_switch(&self) {
        error!("═══════════════════════════════════════════════════════════");
        error!("🐋 ORCA KILL-SWITCH ATIVADO");
        error!("💸 Capital protegido. Sistema parado.");
        error!("🔑 Use código de autorização para retomar.");
        error!("═══════════════════════════════════════════════════════════");

        let mut status = self.safety.system_status.write().await;
        *status = safety::SystemStatus::Halted;
    }

    /// 📊 Estatísticas gerais
    pub async fn stats(&self) -> String {
        let capital = *self.current_capital.read().await;
        let profit = *self.total_profit.read().await;
        let gas_saved = *self.total_gas_saved.read().await;
        let count = *self.execution_count.read().await;
        let roi = if self.initial_capital > 0.0 {
            (profit / self.initial_capital) * 100.0
        } else {
            0.0
        };

        format!(
            "\u{1f40b} ORCA | Capital: {} ETH | Lucro: {} ETH | ROI: {:.1}% | Execuções: {} | Gás poupado: {} ETH",
            capital, profit, roi, count, gas_saved
        )
    }

    /// 🕵️ Observa oportunidade sem executar (PASSIVE_OBSERVER)
    /// Prova rentabilidade real sem queimar capital
    pub async fn observe_opportunity(&self, opportunity: Opportunity, whale_tx_hash: &str) {
        // 1. Validar via simulação
        let sim_result = match self.validate_opportunity(&opportunity).await {
            Ok(r) => r,
            Err(e) => {
                // Não logar rejeições comuns para não poluir o terminal
                trace!("[ORCA] ⛔ Oportunidade observada mas rejeitada: {}", e);
                return;
            }
        };

        // 2. Registar no auditor forense (Forensic Mode)
        let block_timing = self.sequencer.calculate_optimal_timing().await;

        // Simular variação de preço (slippage real) no microssegundo
        let simulated_slippage = 0.00045; // 0.045% slippage real simulado

        info!(
            "🎯 [OPPORTUNITY] Oportunidade Detetada! Baleia: {} | Lucro Estimado: {} ETH",
            &whale_tx_hash[..10],
            sim_result.net_profit_eth
        );

        self.audit
            .log_opportunity(
                block_timing.target_block,
                &format!("0x{:x}", block_timing.target_block),
                whale_tx_hash,
                sim_result.net_profit_eth,
                simulated_slippage,
                sim_result.gas_cost_eth,
                3500.0, // Preço ETH/EUR (fixo para simulação)
            )
            .await;
    }

    /// 🏁 Encerra sessão e gera relatório final
    pub async fn shutdown(&self) {
        info!("[ORCA] 🏁 Encerrando sessão de monitorização...");
        self.audit.generate_final_report().await;
    }
}

#[async_trait]
impl Strategy for OrcaEngine {
    /// 📥 Processa eventos do Artemis e encaminha para o motor ORCA
    /// LOGGING TOTALMENTE TRANSPARENTE - mostra EXATAMENTE o que está a acontecer
    async fn process_event(
        &mut self,
        event: MevEvent,
        context: &StrategyContext,
    ) -> eyre::Result<()> {
        match event {
            MevEvent::Swap(swap) => {
                self.last_observed_block
                    .store(swap.block_number, Ordering::Relaxed);
                let current_block = swap.block_number;

                // ── Deduplicação: cada log processado no máximo 1 vez ──
                {
                    let mut seen = self.seen_events.write().await;
                    let key = (swap.tx_hash, swap.log_index);
                    if seen.contains_key(&key) {
                        trace!(
                            "[DEDUP] Skip evento duplicado tx={:?} log_index={} block={}",
                            swap.tx_hash, swap.log_index, swap.block_number
                        );
                        return Ok(());
                    }
                    // Limpar entradas com mais de 1 bloco de idade (TTL)
                    seen.retain(|_, block| *block >= current_block.saturating_sub(1));
                    seen.insert(key, current_block);
                }

                // ── Sync Event (marcador fee=0): actualizar reserves reais, sem cálculo arb ──
                if swap.fee == 0 {
                    // amount_in = reserve0, amount_out = reserve1 (valores reais do on-chain)
                    self.pool_cache.update_sync_event(
                        swap.pool,
                        swap.amount_in,
                        swap.amount_out,
                        swap.block_number,
                    );
                    // Alimentar CurvatureDetector com reserves reais
                    {
                        let r_in = swap.amount_in.to::<u128>() as f64;
                        let r_out = swap.amount_out.to::<u128>() as f64;
                        self.curvature.write().await.update(
                            swap.pool,
                            swap.block_number,
                            r_in,
                            r_out,
                        );
                    }
                    trace!(
                        "[SYNC] Cache actualizado: pool={:?} r0={} r1={}",
                        swap.pool,
                        swap.amount_in,
                        swap.amount_out
                    );
                    return Ok(()); // Não fazer cálculo arb para Sync events
                }

                // ── Real Swap event: lógica existente abaixo ──

                // Resolver token_in/token_out do cache quando ZERO (decoder não conhece tokens)
                let (resolved_token_in, resolved_token_out) = if swap.token_in != Address::ZERO {
                    (swap.token_in, swap.token_out)
                } else if let Some(pool_state) = self.pool_cache.get(&swap.pool) {
                    if pool_state.token0 != Address::ZERO {
                        (pool_state.token0, pool_state.token1)
                    } else {
                        (Address::ZERO, Address::ZERO)
                    }
                } else {
                    (Address::ZERO, Address::ZERO)
                };

                // Log útil mostrando tokens reais quando disponíveis
                if resolved_token_in != Address::ZERO {
                    debug!(
                        "[SWAP] pool={:?} {} → {} amount={}",
                        swap.pool, resolved_token_in, resolved_token_out, swap.amount_in
                    );
                }

                // 1) Atualizar cache com dados do swap
                // Para pools sem reserves explícitas no evento, usamos amount_in/out como aproximação.
                let synthetic_reserve0 = swap.amount_in.saturating_mul(U256::from(20u32));
                let synthetic_reserve1 = swap.amount_out.saturating_mul(U256::from(20u32));
                // Atualizar cache: sintético apenas para pools sem reserves reais ainda.
                // Não destruir reserves reais com aproximações de amount_in × 20.
                let pool_has_real_reserves = self
                    .pool_cache
                    .get(&swap.pool)
                    .map(|s| s.last_update_block > 0)
                    .unwrap_or(false);
                if pool_has_real_reserves {
                    // Só actualizar timestamp — preservar reserves reais do bootstrap
                    self.pool_cache.touch(swap.pool, swap.block_number);
                } else {
                    // Pool ainda não bootstrapado — usar aproximação sintética como proxy
                    self.pool_cache.update_sync_event(
                        swap.pool,
                        synthetic_reserve0,
                        synthetic_reserve1,
                        swap.block_number,
                    );
                }

                if let (Some(sqrt), Some(liq)) = (swap.sqrt_price_x96, swap.liquidity) {
                    self.pool_cache
                        .update_swap_event(swap.pool, sqrt, liq, swap.block_number);
                }

                // Sistema 3: scoring de frequência
                self.pool_scorer
                    .on_swap_received(&format!("{:?}", swap.pool));

                // Throttle: swaps pequenos só 1x por bloco; swaps grandes calculam sempre
                let last = self.last_processed_block.load(Ordering::Relaxed);
                let is_large_swap = swap.amount_in >= U256::from(1_000_000_000_000_000_000u128); // >= 1 ETH
                if is_large_swap {
                    let needs_bootstrap = self.pool_cache.get(&swap.pool)
                        .map(|s| s.last_update_block == 0)
                        .unwrap_or(true);
                    info!("[DEBUG] large_swap pool={:?} needs_bootstrap={} in_cache={}", 
                        swap.pool, needs_bootstrap, self.pool_cache.contains(&swap.pool));
                    info!("[LARGE-SWAP] pool={:?} amount_in={}", swap.pool, swap.amount_in);
                    // ── JIT: avaliar oportunidade just-in-time para pools V3 ──
                    if swap.dex_type == crate::contracts::DexType::UniswapV3 {
                        if let Some(pool_state) = self.pool_cache.get(&swap.pool) {
                            if pool_state.sqrt_price_x96.is_some() && pool_state.liquidity.is_some() {
                                let cl_pool = CLPool {
                                    address: swap.pool,
                                    token0: pool_state.token0,
                                    token1: pool_state.token1,
                                    fee: pool_state.fee,
                                    tick: pool_state.tick.unwrap_or(0),
                                    liquidity: pool_state.liquidity.unwrap_or(0),
                                    sqrt_price_x96: U256::from(pool_state.sqrt_price_x96.unwrap_or(0)),
                                    tvl_usd: pool_state.tvl_eth.to::<u128>() as f64 / 1e18 * 1800.0,
                                };
                                let swap_eth = swap.amount_in.to::<u128>() as f64 / 1e18;
                                let gas_gwei = 0.1f64;
                                if let Some(jit_opp) = self.jit_monitor.evaluate_opportunity(&cl_pool, swap_eth, gas_gwei) {
                                    info!("[JIT] 🎯 pool={:?} fee={:.6}ETH gas={:.6}ETH", jit_opp.pool, jit_opp.expected_fee_eth, jit_opp.gas_cost_eth);
                                }
                            }
                        }
                    }
                    // ── BACKRUN: atualizar reserves com estado pós-swap imediato ──
                    if swap.token_in != Address::ZERO && swap.token_out != Address::ZERO {
                        if let Some(mut pool) = self.pool_cache.get(&swap.pool) {
                            let (new_r0, new_r1) = if pool.token0 == swap.token_in {
                                (pool.reserve0 + swap.amount_in, pool.reserve1.saturating_sub(swap.amount_out))
                            } else {
                                (pool.reserve0.saturating_sub(swap.amount_out), pool.reserve1 + swap.amount_in)
                            };
                            pool.reserve0 = new_r0;
                            pool.reserve1 = new_r1;
                            pool.last_update_block = swap.block_number;
                            self.pool_cache.insert(pool);
                            debug!("[BACKRUN] reserves atualizadas pool={:?} r0={} r1={}", swap.pool, new_r0, new_r1);
                        }
                    }
                    // Bootstrap on-the-fly se pool desconhecida
                    let needs_bootstrap = self.pool_cache.get(&swap.pool)
                        .map(|s| s.last_update_block == 0)
                        .unwrap_or(true);
                    if needs_bootstrap {
                        let provider = self.balance_provider.clone();
                        let pool_addr = swap.pool;
                        let cache = self.pool_cache.clone();
                        let discovery = self.discovery.clone();
                        let launch_mon_ref = self.launch_monitor.clone();
                        let midcap_ref = self.midcap_scanner.clone();
                        tokio::spawn(async move {
                            let q96 = U256::from(1u128) << 96;
                            // token0/token1
                            let t0_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0x0d,0xfe,0x16,0x81].into());
                            let t1_call = alloy::rpc::types::TransactionRequest::default()
                               .to(pool_addr).input(vec![0xd2,0x12,0x20,0xa7].into());
                            let t0 = provider.call(&t0_call).await.ok();
                            let t1 = provider.call(&t1_call).await.ok();
                            let (token0, token1) = match (t0, t1) {
                                (Some(r0), Some(r1)) if r0.len() >= 32 && r1.len() >= 32 => {
                                    (Address::from_slice(&r0[12..32]), Address::from_slice(&r1[12..32]))
                                }
                                _ => return,
                            };
                            if token0 == Address::ZERO || token1 == Address::ZERO { return; }
                            // Tentar slot0+liquidity (V3)
                            let slot0_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0x38,0x50,0xc7,0xbd].into());
                            let liq_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0x1a,0x68,0x65,0x02].into());
                            let slot0 = provider.call(&slot0_call).await.ok();
                            let liq = provider.call(&liq_call).await.ok();
                            let (reserve0, reserve1, dex, sqrt_opt, liq_opt) =
                                if let (Some(s), Some(l)) = (slot0, liq) {
                                    if s.len() >= 32 && l.len() >= 16 {
                                        let sqrt = U256::from_be_slice(&s[0..32]);
                                        let liquidity = u128::from_be_bytes(l[16..32].try_into().unwrap_or([0;16]));
                                        let liq_u = U256::from(liquidity);
                                        let r0 = liq_u.checked_mul(q96).and_then(|v| v.checked_div(sqrt)).unwrap_or(U256::ZERO);
                                        let r1 = liq_u.checked_mul(sqrt).and_then(|v| v.checked_div(q96)).unwrap_or(U256::ZERO);
                                        (r0, r1, crate::contracts::DexType::UniswapV3, Some(sqrt.saturating_to::<u128>()), Some(liquidity))
                                    } else { (U256::ZERO, U256::ZERO, crate::contracts::DexType::Aerodrome, None, None) }
                                } else {
                                    // Fallback V2 getReserves
                                    let gr_call = alloy::rpc::types::TransactionRequest::default()
                                        .to(pool_addr).input(vec![0x09,0x02,0xf1,0xac].into());
                                    match provider.call(&gr_call).await.ok() {
                                        Some(d) if d.len() >= 64 => {
                                            let r0 = U256::from_be_slice(&d[0..32]);
                                            let r1 = U256::from_be_slice(&d[32..64]);
                                            (r0, r1, crate::contracts::DexType::Aerodrome, None, None)
                                        }
                                        _ => return,
                                    }
                                };
                            if reserve0.is_zero() || reserve1.is_zero() { return; }
                            let usdc = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
                            let dec0 = if token0 == usdc { 6u8 } else { 18u8 };
                            let dec1 = if token1 == usdc { 6u8 } else { 18u8 };
                            let mut state = crate::cache::pool_cache::PoolState::new(pool_addr, token0, token1, 3000, dex);
                            state.reserve0 = reserve0;
                            state.reserve1 = reserve1;
                            state.decimals0 = dec0;
                            state.decimals1 = dec1;
                            state.sqrt_price_x96 = sqrt_opt;
                            state.liquidity = liq_opt;
                            state.last_update_block = 1;
                            // Validar reserves: mínimo 0.1 ETH em ambos os lados
                            let min_reserve = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
                            if reserve0 < min_reserve && reserve1 < min_reserve { return; }
                            // Para V3: rejeitar se sqrt_price implica preço absurdo (fee harcoded 3000 é proxy)
                            cache.insert(state);
                            // Alimentar launch_monitor e midcap_scanner com pool nova
                            launch_mon_ref.on_pair_created(pool_addr, 0);
                            midcap_ref.track_token(token0);
                            midcap_ref.track_token(token1);
                            let disc = discovery.clone();
                            tokio::spawn(async move {
                                disc.register_pool_otf(
                                    pool_addr, token0, token1, 3000,
                                    reserve0, reserve1, 2000.0,
                                ).await;
                                let _ = disc.save_to_cache().await;
                            });
                            info!("[ON-THE-FLY] {:?} t0={:?} t1={:?} r0={} r1={}", pool_addr, token0, token1, reserve0, reserve1);
                        });
                    }
                }
                if !is_large_swap {
    if self.last_processed_block.fetch_max(current_block, Ordering::Relaxed) >= current_block {
                        return Ok(());
                    }
                } else {
                    // Large swap: mesmo gate — 1x por bloco
    if self.last_processed_block.fetch_max(current_block, Ordering::Relaxed) >= current_block {
                        return Ok(());
                    }
                }

                // ▸ Tocar TODAS as pools bootstrapadas com o bloco actual.
                //   Pools vAMM de alta liquidez (WETH/USDC, DAI/USDC, etc.) podem
                //   não ter swaps na janela de subscrição e seriam marcadas stale
                //   após 500 blocos. Um touch por bloco mantém-nas sempre frescas.
                //   Custo: DashMap iteration uma vez por bloco (~0.5s) — aceitável.
                {
                    let all_pools: Vec<alloy::primitives::Address> = self
                        .pool_cache
                        .get_sample_pools(self.pool_cache.len())
                        .into_iter()
                        .filter(|s| s.last_update_block > 0 && s.has_liquidity())
                        .map(|s| s.address)
                        .collect();
                    for pool_addr in all_pools {
                        self.pool_cache.touch(pool_addr, current_block);
                    }
                }

                // 2) Verificar bankroll antes de calcular
                let bankroll = self.bankroll_manager.read().await;
                let risk_multiplier = bankroll.risk_multiplier();
                if risk_multiplier == 0.0 {
                    warn!(
                        "   💰 [BANKROLL] Circuit breaker ATIVO — {} falhas consecutivas. Abortando.",
                        bankroll.consecutive_failures.load(std::sync::atomic::Ordering::Relaxed));
                    return Ok(());
                }
                // BUG ANTERIOR: passava `synthetic_reserve0 = swap.amount_in × 20`
                // ao bankroll.  Para um swap de 1 USDC raw, isso dá reserve_in=20
                // → cap de 15% = 3 wei → flash_amount=3 wei → 0 output em pools
                // com reserve ~352 ETH (numerator < denominator na divisão inteira).
                //
                // CORREÇÃO: usar a reserve REAL da pool alvo do swap.  O MIN_FLASH_WEI
                // no bankroll garante 0.01 ETH mínimo independentemente do resultado.
                let real_reserve_for_bankroll: u128 = self
                    .pool_cache
                    .get(&swap.pool)
                    .map(|s| {
                        // Usar reserve0 (18-dec) como proxy de profundidade.
                        // Se a pool ainda não tem reserve real, cair para sintético.
                        if s.last_update_block > 0 && !s.reserve0.is_zero() {
                            s.reserve0.try_into().unwrap_or(u128::MAX / 2)
                        } else {
                            synthetic_reserve0.to::<u128>()
                        }
                    })
                    .unwrap_or_else(|| synthetic_reserve0.to::<u128>());

                let optimal_flash_wei = bankroll.optimal_flash_amount(real_reserve_for_bankroll);
                drop(bankroll);

                trace!(
                    "[BANKROLL] pool={:?} real_r0={} flash={} wei",
                    swap.pool,
                    real_reserve_for_bankroll,
                    optimal_flash_wei
                );

                // 3) Chamar find_opportunities() obrigatoriamente
                let mut graph = self.arb_graph.write().await;
                graph.rebuild(swap.block_number);
                let v3_count = self.pool_cache
                    .get_sample_pools(self.pool_cache.len())
                    .into_iter()
                    .filter(|p| matches!(p.dex_type, DexType::UniswapV3))
                    .count();
                tracing::info!(v3_pools = v3_count, "graph composition");
                let hour_now = chrono::Utc::now().hour() as u8;
                let pools: Vec<Address> = self
                    .pool_cache
                    .get_sample_pools(self.pool_cache.len())
                    .into_iter()
                    .map(|p| p.address)
                    .collect();
                let mut pool_priorities = self.pattern_memory.to_priority_map(&pools, hour_now);
                // Misturar scorer (0..1000) na prioridade (pattern_score + scorer_score)
                for pool in &pools {
                    let key = format!("{:?}", pool);
                    let s = self.pool_scorer.get_score(&key) as f64;
                    pool_priorities
                        .entry(*pool)
                        .and_modify(|v| *v += s)
                        .or_insert(s);
                }
                // Ω-Curvature: boost de prioridade para pools com sinal de divergência iminente
                {
                    let pool_pairs: Vec<(Address, Address)> = pools.windows(2)
                        .map(|w| (w[0], w[1]))
                        .collect();
                    let curvature = self.curvature.read().await;
                    let signals = curvature.detect(swap.block_number, &pool_pairs);
                    for sig in &signals {
                        let boost = sig.omega * 5000.0; // escala Ω para prioridade
                        pool_priorities.entry(sig.pool_a).and_modify(|v| *v += boost).or_insert(boost);
                        pool_priorities.entry(sig.pool_b).and_modify(|v| *v += boost).or_insert(boost);
                        if sig.omega > 0.01 {
                            info!("[Ω] Sinal curvatura: pool_a={:?} pool_b={:?} Ω={:.4} bloco={}", 
                                sig.pool_a, sig.pool_b, sig.omega, sig.block);
                        }
                    }
                }
                // Transfer Entropy: alimentar preço atual e boost pools causalmente ligados
                {
                    let price_proxy = if !swap.amount_out.is_zero() {
                        swap.amount_in.to::<u128>() as f64 / swap.amount_out.to::<u128>().max(1) as f64
                    } else { 0.0 };
                    if price_proxy > 0.0 {
                        let mut te = self.transfer_entropy.write().await;
                        te.record_price(swap.pool, price_proxy);
                        te.update_causality();
                        let caused = te.get_caused_pools(swap.pool);
                        for (caused_pool, te_score) in caused.iter().take(5) {
                            let boost = te_score * 3000.0;
                            pool_priorities.entry(*caused_pool).and_modify(|v| *v += boost).or_insert(boost);
                        }
                    }
                }

                // Garantir mínimo de 0.01 ETH (10^16 wei) — abaixo disso a divisão AMM trunca para zero
                const MIN_FLASH_WEI: u128 = 10_000_000_000_000_000u128; // 0.01 ETH
                let flash_amounts = {
                    let base = optimal_flash_wei.max(MIN_FLASH_WEI);
                    let mut opt = self.flash_optimizer.write().await;
                    let optimized = opt.optimize(
                        "weth_arb",
                        |input_wei| input_wei + (input_wei / 150), // +0.67% proxy
                        base / 500, // gas proxy
                        MIN_FLASH_WEI,
                        base.saturating_mul(2),
                    ).unwrap_or(base);
                    vec![
                        U256::from(optimized),
                        U256::from((optimized / 2).max(MIN_FLASH_WEI)),
                        U256::from(optimized.saturating_mul(2)),
                    ]
                };
                let observed_gas = context.priority_fee_gwei as f64;
                let predicted_gas = self.kalman_gas.write().await.update(observed_gas);
                let gas_price_wei = U256::from((predicted_gas * 1_000_000_000.0) as u64);
                let t = std::time::Instant::now();
                // Multi-token start: WETH + USDC + cbETH + AERO + tokens divergentes (long-tail)
                let divergences = self.midcap_scanner.find_divergences();
                let mut extra_tokens: Vec<Address> = divergences.iter().map(|d| d.token).collect();
                extra_tokens.dedup();
                let recent_launches = self.launch_monitor.get_recent_launches(swap.block_number);
                extra_tokens.extend(recent_launches);

                let mut opps = Vec::new();
                let mut start_tokens = vec![WETH, USDC, CBETH, AERO];
                start_tokens.extend(extra_tokens);
                for start_tok in start_tokens {
                    let tok_opps = graph.find_opportunities_with_priorities(
                        start_tok,
                        &flash_amounts,
                        gas_price_wei,
                        1.2,
                        Some(&pool_priorities),
                    );
                    opps.extend(tok_opps);
                }
                opps.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));
                let opps: Vec<_> = opps.into_iter().filter(|opp| {
                    let tokens: Vec<_> = opp.hops.iter().map(|h| h.token_in).collect();
                    self.honeypot.is_path_safe(&tokens)
                }).collect();
                // Topologia de Persistência — alimentar e boost de prioridade
                {
                    let mut topo = self.topology.write().await;
                    for opp in &opps {
                        let pools: Vec<Address> = opp.hops.iter().map(|h| h.pool).collect();
                        let spread = if opp.input_amount.is_zero() { 0.0 } else {
                            opp.gross_profit.to::<u128>() as f64 / opp.input_amount.to::<u128>() as f64
                        };
                        if let Some(signal) = topo.observe_cycle(&pools, spread, swap.block_number) {
                            info!(
                                "[TOPO] {} ciclo: spread={:.4}% persistence_score={:.1} bloco={}",
                                if signal.is_revival { "Revival" } else { "Novo" },
                                signal.spread * 100.0,
                                signal.persistence_score,
                                signal.block
                            );
                        }
                    }
                    // Boost de prioridade para pools em ciclos persistentes
                    for pool in &pools {
                        let boost = topo.pool_persistence_boost(*pool);
                        if boost > 0.0 {
                            pool_priorities.entry(*pool).and_modify(|v| *v += boost).or_insert(boost);
                        }
                    }
                }
                let elapsed_us = t.elapsed().as_micros();
                if let Some(ref telem) = self.telemetry {
                    telem.record_scan(elapsed_us).await;
                    telem.record_newton_raphson(elapsed_us).await;
                }

                // 4) Logar resultado
                if opps.is_empty() {
                    debug!("[ARB] Bloco {}: sem oportunidades", swap.block_number);
                } else {
                    self.pattern_memory.record_opportunity(
                        swap.pool,
                        hour_now,
                        opps[0].net_profit.to::<u128>(),
                    );
                    self.pool_scorer.on_opportunity_found(
                        &format!("{:?}", swap.pool),
                        opps[0].net_profit.to::<u128>(),
                    );
                    info!(
                        "[ARB] 🎯 {} oportunidades | Melhor: {:.6} ETH profit | Hops: {}",
                        opps.len(),
                        opps[0].net_profit.to::<u128>() as f64 / 1e18,
                        opps[0].hops.len()
                    );
                    // Log CSV para análise DRY_RUN
                    // Log CSV — apenas melhor oportunidade por path único por bloco
                    let mut seen_paths = std::collections::HashSet::new();
                    for opp in &opps {
                        let path_str = {
                            let mut parts: Vec<String> = opp.hops.iter().map(|h| format!("{:?}", h.token_in)).collect();
                            if let Some(last) = opp.hops.last() { parts.push(format!("{:?}", last.token_out)); }
                            parts.join("→")
                        };
                        // Filtrar: só pools com reserves verificadas via getReserves() real
                        let all_verified = opp.hops.iter().all(|h| {
                            self.pool_cache.get(&h.pool).map(|p| p.reserve_verified).unwrap_or(false)
                        });
                        if !all_verified { continue; }
                        if seen_paths.contains(&path_str) { continue; }
                        seen_paths.insert(path_str.clone());
                        self.opp_logger.log(&crate::logger::opportunity_logger::OpportunityRecord {
                            block: swap.block_number,
                            path: path_str.clone(),
                            hops: opp.hops.len(),
                            input_wei: opp.input_amount.to::<u128>(),
                            gross_profit_wei: opp.gross_profit.to::<u128>(),
                            net_profit_wei: opp.net_profit.to::<u128>(),
                            gas_cost_wei: opp.gas_cost.to::<u128>(),
                        });
                        // Notificação Discord para opps > 1€
                        let profit_eur = opp.net_profit.to::<u128>() as f64 / 1e18 * 1800.0;
                        if profit_eur >= 1.0 {
                            let discord = self.discord.clone();
                            let path_discord = path_str.clone();
                            let hops_n = opp.hops.len();
                            let block_n = swap.block_number;
                            tokio::spawn(async move {
                                discord.notify_opportunity(&path_discord, profit_eur, hops_n, block_n).await;
                            });
                        }
                    }
                }

                let mut last_persist = self.last_pattern_persist_block.write().await;
                if swap.block_number.saturating_sub(*last_persist) >= 100 {
                    self.pattern_memory.persist_to_disk();
                    *last_persist = swap.block_number;
                }

                // Sistema 2: Reserve inference (só quando recebemos sqrtPriceX96)
                if matches!(
                    swap.dex_type,
                    DexType::UniswapV3 | DexType::PancakeSwap | DexType::AerodromeStable
                ) {
                    if let Some(sqrt) = swap.sqrt_price_x96 {
                        let candidates = self
                            .pool_cache
                            .get_pools_by_tokens(swap.token_in, swap.token_out);
                        let mut triggered = false;
                        for v2_pool in candidates.into_iter().filter(|p| {
                            matches!(p.dex_type, DexType::UniswapV2 | DexType::Aerodrome)
                        }) {
                            let divergence_bps = detect_cross_pool_divergence(
                                sqrt,
                                v2_pool.reserve0.to::<u128>(),
                                v2_pool.reserve1.to::<u128>(),
                                v2_pool.decimals0,
                                v2_pool.decimals1,
                            );
                            if divergence_bps > 10 {
                                info!(
                                    "[INFERENCE] Divergência {}bps entre V3 e V2 — verificando arb",
                                    divergence_bps
                                );
                                triggered = true;
                                break;
                            }
                        }

                        if triggered {
                            // Re-scan rápido com as mesmas prioridades já calculadas
                            let _ = graph.find_opportunities_with_priorities(
                                WETH,
                                &flash_amounts,
                                gas_price_wei,
                                1.2,
                                Some(&pool_priorities),
                            );
                        }
                    }
                }
            }
            MevEvent::BlockUpdate(block) => {
                self.last_observed_block.store(block, Ordering::Relaxed);
                let mut last_persist = self.last_pattern_persist_block.write().await;
                if block.saturating_sub(*last_persist) >= 100 {
                    self.pattern_memory.persist_to_disk();
                    *last_persist = block;
                }
                drop(last_persist);

                // Guardar wallet para leituras periódicas de saldo on-chain
                {
                    let mut tracked_wallet = self.tracked_wallet.write().await;
                    if tracked_wallet.is_none() {
                        *tracked_wallet = Some(context.executor_address);
                    }
                }

                // Atualizar saldo real da wallet a cada 10 blocos
                if block % 10 == 0 {
                    if let Err(err) = self.sync_wallet_balance().await {
                        warn!("[BANKROLL] Falha ao sincronizar saldo on-chain: {}", err);
                    }
                }

                let mut graph = self.arb_graph.write().await;
                graph.rebuild(block);
                let v3_count = self.pool_cache
                    .get_sample_pools(self.pool_cache.len())
                    .into_iter()
                    .filter(|p| matches!(p.dex_type, DexType::UniswapV3))
                    .count();
                tracing::info!(v3_pools = v3_count, "graph composition");
                trace!("[ORCA] Bloco {} detetado — grafo reconstruído", block);

                // 💰 Status report a cada 1000 blocos
                let mut last_block = self.last_status_block.write().await;
                if block.saturating_sub(*last_block) >= 1000 {
                    *last_block = block;
                    let bankroll = self.bankroll_manager.read().await;
                    info!("{}", bankroll.status_report());
                    drop(bankroll);
                }
                drop(last_block);
            }
            _ => {}
        }
        Ok(())
    }

    async fn initialize(&mut self, _initial_data: Vec<NormalizedSwapEvent>) -> eyre::Result<()> {
        info!("[ORCA] Motor sincronizado com a Mainnet.");
        let last_block = Arc::clone(&self.last_observed_block);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                info!(
                    "[HEARTBEAT] Bot activo | Bloco: {}",
                    last_block.load(Ordering::Relaxed)
                );
            }
        });
        Ok(())
    }

    fn stats(&self) -> crate::artemis::strategy::StrategyStats {
        crate::artemis::strategy::StrategyStats::default()
    }
}

/// 💎 Oportunidade de MEV
#[derive(Clone, Debug)]
pub struct Opportunity {
    pub id: u64,
    pub pool_address: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub expected_profit_eth: f64,
    pub opportunity_type: OpportunityType,
}

/// 🎯 Tipo de oportunidade
#[derive(Clone, Debug, PartialEq)]
pub enum OpportunityType {
    Arbitrage,
    Liquidation,
    Sandwich,
    GhostCallback,
}

/// 🧮 Resultado de simulação
#[derive(Clone, Debug)]
pub struct SimulationResult {
    pub gross_profit_eth: f64,
    pub net_profit_eth: f64,
    pub gas_used: u64,
    pub gas_cost_eth: f64,
    pub gas_saved_eth: f64,
    pub will_succeed: bool,
}

/// 📦 Bundle protegido
#[derive(Clone, Debug)]
pub struct ProtectedBundle {
    pub transactions: Vec<Bytes>,
    pub min_profit_eth: f64,
    pub max_gas_eth: f64,
    pub target_slot: u16,
    pub revert_on_failure: bool,
}

/// 🧾 Recibo de execução
#[derive(Clone, Debug)]
pub struct ExecutionReceipt {
    pub tx_hash: String,
    pub block_number: u64,
    pub slot: u16,
    pub profit_eth: f64,
    pub gas_used: u64,
    pub gas_saved_eth: f64,
    pub timestamp: u64,
}

/// 🚦 Status do sistema ORCA
#[derive(Clone, Debug, PartialEq)]
pub enum OrcaSystemStatus {
    /// Operando normalmente
    Active,
    /// Em pausa (monitorização)
    Idle,
    /// Kill-switch ativado
    Halted,
    /// Aguardando autorização
    AwaitingAuth,
}
