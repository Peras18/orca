//! CONTINUOUS PROFIT ENGINE - Lucro Recorrente sem Silêncio
//!
//! Funcionalidades:
//! 1. Reduce Threshold - Lucro mínimo adaptativo para backrunning
//! 2. Continuous Mempool Scan - Reage a pending txs, não blocos
//! 3. Recursive Pathing - Recalcula imediatamente após trade
//! 4. Gas Sensitivity - Primeiros em trades pequenos de alta frequência
//!
//! Target: Zero períodos de silêncio, lucro contínuo 24/7

use alloy::primitives::{Address, U256, FixedBytes};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
// use tokio::time::interval; // Not used currently
use tracing::{info, debug, trace};

use crate::strategist::{
    apex_predator::ApexPredator,
    newton_jacobian_solver::NewtonJacobianSolver,
};
use crate::executor::gas_auction::GasAuctionController;

/// 🎯 CONFIGURAÇÃO DE THRESHOLD ADAPTATIVO
pub const REDUCED_PROFIT_THRESHOLD_SMALL: f64 = 5.0;    // $5 para trades pequenos
pub const REDUCED_PROFIT_THRESHOLD_MEDIUM: f64 = 10.0;  // $10 para trades médios
pub const REDUCED_PROFIT_THRESHOLD_LARGE: f64 = 20.0;   // $20 para trades grandes

/// 📊 THRESHOLD DINÂMICO BASEADO EM PROBABILIDADE
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TradeProbability {
    VeryHigh,  // > 95% - Aceita lucro > $5
    High,      // > 85% - Aceita lucro > $10
    Medium,    // > 70% - Aceita lucro > $15
    Low,       // > 50% - Aceita lucro > $20
}

impl TradeProbability {
    pub fn min_profit(&self) -> f64 {
        match self {
            TradeProbability::VeryHigh => REDUCED_PROFIT_THRESHOLD_SMALL,   // $5
            TradeProbability::High => REDUCED_PROFIT_THRESHOLD_MEDIUM,     // $10
            TradeProbability::Medium => 15.0,                                // $15
            TradeProbability::Low => REDUCED_PROFIT_THRESHOLD_LARGE,        // $20
        }
    }
    
    pub fn from_confidence(confidence: f64) -> Self {
        if confidence >= 0.95 {
            TradeProbability::VeryHigh
        } else if confidence >= 0.85 {
            TradeProbability::High
        } else if confidence >= 0.70 {
            TradeProbability::Medium
        } else {
            TradeProbability::Low
        }
    }
}

/// 🔄 CONTINUOUS ENGINE
pub struct ContinuousProfitEngine {
    /// Modo de operação contínua
    continuous_mode: Arc<RwLock<bool>>,
    
    /// Threshold adaptativo atual
    current_threshold: Arc<RwLock<f64>>,
    
    /// Contador de trades por minuto
    trades_per_minute: Arc<RwLock<u32>>,
    
    /// Último trade executado
    last_trade_time: Arc<RwLock<Instant>>,
    
    /// Fila de oportunidades recursivas
    recursive_queue: Arc<RwLock<VecDeque<RecursiveOpportunity>>>,
    
    /// Canal para sinalização de novo bloco
    block_rx: mpsc::Receiver<u64>,
    
    /// Canal para sinalização de nova pending tx
    pending_tx_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<PendingTxInfo>>>,
    
    /// Referência ao solver
    solver: Arc<NewtonJacobianSolver>,
    
    /// Referência ao gas controller
    gas_controller: Arc<GasAuctionController>,
    
    /// Referência ao apex predator
    apex: Arc<ApexPredator>,
}

/// 📨 Informação de Transação Pendente
#[derive(Clone, Debug)]
pub struct PendingTxInfo {
    pub tx_hash: FixedBytes<32>,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    pub gas_price: U256,
    pub data: Vec<u8>,
    pub detected_at: Instant,
    pub is_whale: bool,
    pub whale_size_eth: f64,
}

/// 🔄 Oportunidade Recursiva
#[derive(Clone, Debug)]
pub struct RecursiveOpportunity {
    pub pool_address: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub previous_profit: f64,
    pub recursion_depth: u8,
    pub max_recursions: u8,
    pub timestamp: Instant,
}

/// ⛽ GAS SENSITIVITY CONTROLLER
pub struct GasSensitivityController {
    /// Modo de alta frequência
    high_frequency_mode: Arc<RwLock<bool>>,
    
    /// Contador de trades pequenos (< $10)
    small_trade_count: Arc<RwLock<u32>>,
    
    /// Gas mínimo para ser primeiro
    min_gas_to_be_first: Arc<RwLock<u64>>, // gwei
    
    /// Ajuste dinâmico de gas
    gas_adjustment_factor: Arc<RwLock<f64>>,
}

impl GasSensitivityController {
    pub fn new() -> Self {
        Self {
            high_frequency_mode: Arc::new(RwLock::new(true)),
            small_trade_count: Arc::new(RwLock::new(0)),
            min_gas_to_be_first: Arc::new(RwLock::new(50)), // 50 gwei default
            gas_adjustment_factor: Arc::new(RwLock::new(1.0)),
        }
    }
    
    /// 🎯 Calcula gas para trades pequenos de alta frequência
    pub async fn calculate_sensitive_gas(
        &self,
        expected_profit_usd: f64,
        competitor_gas_gwei: u64,
        is_small_trade: bool,
    ) -> u64 {
        let base_gas = *self.min_gas_to_be_first.read().await;
        let factor = *self.gas_adjustment_factor.read().await;
        
        if is_small_trade && expected_profit_usd < 10.0 {
            // Trade pequeno: gas agressivo para ser primeiro
            let aggressive_gas = (competitor_gas_gwei as f64 * 1.5 * factor) as u64;
            let max_affordable = (expected_profit_usd * 0.5 / 2500.0 * 1e9 / 150_000.0) as u64;
            
            let final_gas = aggressive_gas.min(max_affordable).max(base_gas);
            
            info!("⚡⚡⚡ [GAS SENSITIVE] Trade pequeno ${:.2} | Gas: {} gwei (vs {} competitor)",
                expected_profit_usd, final_gas, competitor_gas_gwei);
            
            final_gas
        } else {
            // Trade normal: gas padrão
            (competitor_gas_gwei as f64 * 1.1) as u64
        }
    }
    
    /// 📊 Atualiza contador de trades pequenos
    pub async fn record_small_trade(&self, profit_usd: f64) {
        if profit_usd < 10.0 {
            let mut count = self.small_trade_count.write().await;
            *count += 1;
            
            // Ajustar fator baseado na frequência
            let mut factor = self.gas_adjustment_factor.write().await;
            if *count > 10 {
                *factor = 1.2; // 20% mais agressivo
            } else if *count > 20 {
                *factor = 1.5; // 50% mais agressivo
                info!("🔥🔥🔥 [GAS SENSITIVE] Modo ULTRA agressivo ativado! 20+ trades pequenos");
            }
        }
    }
    
    /// 🔄 Reseta contadores (chamar a cada minuto)
    pub async fn reset_counters(&self) {
        let mut count = self.small_trade_count.write().await;
        let old_count = *count;
        *count = 0;
        drop(count);
        
        let mut factor = self.gas_adjustment_factor.write().await;
        *factor = 1.0;
        drop(factor);
        
        if old_count > 0 {
            debug!("[GAS SENSITIVE] Contadores resetados ({} trades/min)", old_count);
        }
    }
}

impl ContinuousProfitEngine {
    pub fn new(
        solver: Arc<NewtonJacobianSolver>,
        gas_controller: Arc<GasAuctionController>,
        apex: Arc<ApexPredator>,
        block_rx: mpsc::Receiver<u64>,
        pending_tx_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<PendingTxInfo>>>,
    ) -> Self {
        Self {
            continuous_mode: Arc::new(RwLock::new(true)),
            current_threshold: Arc::new(RwLock::new(REDUCED_PROFIT_THRESHOLD_SMALL)),
            trades_per_minute: Arc::new(RwLock::new(0)),
            last_trade_time: Arc::new(RwLock::new(Instant::now())),
            recursive_queue: Arc::new(RwLock::new(VecDeque::new())),
            block_rx,
            pending_tx_rx,
            solver,
            gas_controller,
            apex,
        }
    }
    
    /// 🚀 Inicia o engine de lucro contínuo
    pub async fn spawn(self: Arc<Self>) {
        info!("═══════════════════════════════════════════════════════════");
        info!("🔄🔄🔄 CONTINUOUS PROFIT ENGINE - Lucro Sem Silêncio");
        info!("═══════════════════════════════════════════════════════════");
        info!("💰 Threshold Adaptativo: $5 (VeryHigh) → $10 (High) → $20 (Low)");
        info!("⛽ Gas Sensitivity: Ativo para trades < $10");
        info!("🔄 Recursive Pathing: Até 5 recursões por whale");
        info!("⏱️  Mempool Scan: Contínuo (50ms interval)");
        info!("═══════════════════════════════════════════════════════════");
        
        // Spawn mempool scanner (não espera por blocos)
        let engine = self.clone();
        tokio::spawn(async move {
            engine.continuous_mempool_scanner().await;
        });
        
        // Spawn recursive processor
        let engine = self.clone();
        tokio::spawn(async move {
            engine.recursive_processor().await;
        });
        
        // Spawn threshold adjuster
        let engine = self.clone();
        tokio::spawn(async move {
            engine.adaptive_threshold_manager().await;
        });
        
        // Spawn gas sensitivity manager
        let gas_sensitivity = Arc::new(GasSensitivityController::new());
        let gas_clone = gas_sensitivity.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                gas_clone.reset_counters().await;
            }
        });
        
        info!("✅ [CONTINUOUS] Engine iniciado em modo 24/7");
    }
    
    /// 🔄 Scanner contínuo de mempool (não espera blocos)
    async fn continuous_mempool_scanner(&self) {
        let mut last_log = Instant::now();
        let mut scan_count = 0u64;
        
        loop {
            // Verificar pending txs com timeout de 50ms
            let mut rx = self.pending_tx_rx.lock().await;
            match tokio::time::timeout(
                Duration::from_millis(50),
                rx.recv()
            ).await {
                Ok(Some(_tx)) => {
                    scan_count += 1;
                    self.process_pending_transaction(_tx).await;
                }
                Ok(None) => {
                    // Canal fechado
                    break;
                }
                Err(_) => {
                    // Timeout - nenhuma tx pendente, continuar
                }
            }
            
            // Log de status a cada 30 segundos
            if last_log.elapsed() > Duration::from_secs(30) {
                let tpm = *self.trades_per_minute.read().await;
                let threshold = *self.current_threshold.read().await;
                let queue_size = self.recursive_queue.read().await.len();
                
                info!("⏱️  [CONTINUOUS] Status: {} scans/min | Threshold: ${:.2} | Queue: {} | Trades/min: {}",
                    scan_count * 2, threshold, queue_size, tpm);
                
                scan_count = 0;
                last_log = Instant::now();
            }
        }
    }
    
    /// 🎯 Processa transação pendente detectada
    async fn process_pending_transaction(&self, tx: PendingTxInfo) {
        let now = Instant::now();
        
        // Atualizar última atividade
        *self.last_trade_time.write().await = now;
        
        // Calcular probabilidade do trade
        let probability = self.calculate_trade_probability(&tx).await;
        let min_profit = probability.min_profit();
        
        // Ajustar threshold dinamicamente
        *self.current_threshold.write().await = min_profit;
        
        if tx.is_whale && tx.whale_size_eth >= 10.0 {
            info!("🐋🐋🐋 [CONTINUOUS] Whale detectada! {:.2} ETH | Prob: {:?} | MinProfit: ${:.2}",
                tx.whale_size_eth, probability, min_profit);
            
            // Criar oportunidade recursiva
            let recursive_op = RecursiveOpportunity {
                pool_address: tx.to,
                token_in: Address::ZERO, // Decodificar do data
                token_out: Address::ZERO,
                previous_profit: 0.0,
                recursion_depth: 0,
                max_recursions: 5, // Até 5 swaps seguidos da whale
                timestamp: now,
            };
            
            self.recursive_queue.write().await.push_back(recursive_op);
        }
        
        // Processar imediatamente se for oportunidade de alta probabilidade
        if probability == TradeProbability::VeryHigh || probability == TradeProbability::High {
            self.execute_opportunity_if_profitable(&tx, min_profit).await;
        }
    }
    
    /// 🔄 Processador de oportunidades recursivas
    async fn recursive_processor(&self) {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        
        loop {
            interval.tick().await;
            
            // Processar próxima oportunidade na fila
            if let Some(op) = self.recursive_queue.write().await.pop_front() {
                // Verificar se ainda é válida (< 30s desde criação)
                if op.timestamp.elapsed() > Duration::from_secs(30) {
                    continue;
                }
                
                // Recalcular oportunidade imediatamente
                if op.recursion_depth < op.max_recursions {
                    match self.recalculate_opportunity(&op).await {
                        Some(new_op) => {
                            info!("🔄🔄🔄 [RECURSIVE] Recursão {} na pool {:?} | Lucro anterior: ${:.2}",
                                op.recursion_depth, op.pool_address, op.previous_profit);
                            
                            // Guardar valor antes de mover
                            let new_profit = new_op.previous_profit;
                            
                            // Adicionar de volta à fila com próxima recursão
                            let mut next_op = new_op;
                            next_op.recursion_depth = op.recursion_depth + 1;
                            next_op.previous_profit = op.previous_profit + new_profit;
                            
                            // Guardar valor para comparação
                            let accumulated_profit = next_op.previous_profit;
                            
                            let next_op_clone = next_op.clone();
                            self.recursive_queue.write().await.push_back(next_op_clone);
                            
                            // Executar se lucro acumulado > threshold
                            let threshold = *self.current_threshold.read().await;
                            if accumulated_profit > threshold {
                                self.execute_recursive_trade(&next_op.clone()).await;
                            }
                        }
                        None => {
                            trace!("[RECURSIVE] Nenhuma oportunidade recursiva encontrada");
                        }
                    }
                }
            }
        }
    }
    
    /// 📊 Manager de threshold adaptativo
    async fn adaptive_threshold_manager(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        
        loop {
            interval.tick().await;
            
            let tpm = *self.trades_per_minute.read().await;
            let last_trade = *self.last_trade_time.read().await;
            let silence_duration = last_trade.elapsed().as_secs();
            
            // Ajustar threshold baseado na atividade
            let new_threshold = if silence_duration > 300 {
                // Mais de 5 minutos sem trades - REDUZIR threshold
                info!("🔻🔻🔻 [THRESHOLD] Período de silêncio! Reduzindo para $5.00");
                REDUCED_PROFIT_THRESHOLD_SMALL
            } else if tpm < 5 {
                // Poucos trades - reduzir moderadamente
                info!("🔻 [THRESHOLD] Pouca atividade ({} tpm) | Threshold: $10.00", tpm);
                REDUCED_PROFIT_THRESHOLD_MEDIUM
            } else if tpm > 20 {
                // Muitos trades - pode aumentar threshold (seletivo)
                info!("🔺 [THRESHOLD] Alta atividade ({} tpm) | Threshold: $20.00", tpm);
                REDUCED_PROFIT_THRESHOLD_LARGE
            } else {
                // Atividade normal
                REDUCED_PROFIT_THRESHOLD_MEDIUM
            };
            
            *self.current_threshold.write().await = new_threshold;
            
            // Reset contador de trades por minuto
            *self.trades_per_minute.write().await = 0;
        }
    }
    
    /// 🧮 Calcula probabilidade de sucesso de um trade
    async fn calculate_trade_probability(&self, tx: &PendingTxInfo) -> TradeProbability {
        let mut score: f64 = 0.0;
        
        // Whale = alta probabilidade
        if tx.is_whale {
            score += 0.4;
        }
        
        // Gas price alto = competição = oportunidade real
        let gas_gwei = tx.gas_price.to::<u64>() as f64 / 1e9;
        if gas_gwei > 50.0 {
            score += 0.3;
        }
        
        // Value alto = impacto de preço = arbitragem viável
        let value_eth = tx.value.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
        if value_eth > 1.0 {
            score += 0.2;
        }
        
        // Timing (não usamos block time, usamos instant)
        score += 0.1;
        
        TradeProbability::from_confidence(score.min(1.0))
    }
    
    /// 💰 Executa trade se for lucrativo
    async fn execute_opportunity_if_profitable(&self, _tx: &PendingTxInfo, min_profit: f64) {
        // Aqui integraria com o ApexPredator para avaliar
        // Simulação: aceitar se passar do threshold
        
        debug!("[EXECUTE] Avaliando trade com min_profit ${:.2}", min_profit);
        
        // Incrementar contador de trades
        *self.trades_per_minute.write().await += 1;
    }
    
    /// 🔄 Recalcula oportunidade na mesma pool
    async fn recalculate_opportunity(&self, op: &RecursiveOpportunity) -> Option<RecursiveOpportunity> {
        // Simulação: criar nova oportunidade com ligeira variação
        Some(RecursiveOpportunity {
            pool_address: op.pool_address,
            token_in: op.token_in,
            token_out: op.token_out,
            previous_profit: op.previous_profit * 0.9, // Decaimento de 10%
            recursion_depth: op.recursion_depth,
            max_recursions: op.max_recursions,
            timestamp: Instant::now(),
        })
    }
    
    /// 🚀 Executa trade recursivo
    async fn execute_recursive_trade(&self, op: &RecursiveOpportunity) {
        info!("🚀🚀🚀 [RECURSIVE EXEC] Trade recursivo #{} | Pool: {:?} | Lucro acumulado: ${:.2}",
            op.recursion_depth, op.pool_address, op.previous_profit);
        
        // Aqui integraria com o executor real
    }
    
    /// 📊 Retorna estatísticas de lucro contínuo
    pub async fn get_continuous_stats(&self) -> ContinuousStats {
        ContinuousStats {
            continuous_mode: *self.continuous_mode.read().await,
            current_threshold: *self.current_threshold.read().await,
            trades_last_minute: *self.trades_per_minute.read().await,
            recursive_queue_size: self.recursive_queue.read().await.len() as u32,
            seconds_since_last_trade: self.last_trade_time.read().await.elapsed().as_secs() as u32,
        }
    }
}

/// 📊 Estatísticas do modo contínuo
#[derive(Clone, Debug)]
pub struct ContinuousStats {
    pub continuous_mode: bool,
    pub current_threshold: f64,
    pub trades_last_minute: u32,
    pub recursive_queue_size: u32,
    pub seconds_since_last_trade: u32,
}

impl Clone for ContinuousProfitEngine {
    fn clone(&self) -> Self {
        // Criar canais dummy para o clone (os originais não podem ser clonados)
        let (_dummy_block_tx, dummy_block_rx) = mpsc::channel(1);
        let (_dummy_pending_tx, dummy_pending_rx) = mpsc::channel(1);
        
        Self {
            continuous_mode: self.continuous_mode.clone(),
            current_threshold: self.current_threshold.clone(),
            trades_per_minute: self.trades_per_minute.clone(),
            last_trade_time: self.last_trade_time.clone(),
            recursive_queue: self.recursive_queue.clone(),
            block_rx: dummy_block_rx,
            pending_tx_rx: Arc::new(tokio::sync::Mutex::new(dummy_pending_rx)),
            solver: self.solver.clone(),
            gas_controller: self.gas_controller.clone(),
            apex: self.apex.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_trade_probability_thresholds() {
        assert_eq!(TradeProbability::VeryHigh.min_profit(), 5.0);
        assert_eq!(TradeProbability::High.min_profit(), 10.0);
        assert_eq!(TradeProbability::Medium.min_profit(), 15.0);
        assert_eq!(TradeProbability::Low.min_profit(), 20.0);
    }
    
    #[test]
    fn test_probability_from_confidence() {
        assert!(matches!(TradeProbability::from_confidence(0.96), TradeProbability::VeryHigh));
        assert!(matches!(TradeProbability::from_confidence(0.90), TradeProbability::High));
        assert!(matches!(TradeProbability::from_confidence(0.75), TradeProbability::Medium));
        assert!(matches!(TradeProbability::from_confidence(0.60), TradeProbability::Low));
    }
}
