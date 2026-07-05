//! APEX PREDATOR - Economic Engine & Gas War Control
//!
//! Funcionalidades:
//! 1. MIN_PROFIT_PER_TRADE: Filtro de lucro mínimo ($2.00)
//! 2. Gas War Control: Agressividade dinâmica baseada no lucro
//! 3. Profit Accumulator: Tracking de lucro diário
//!
//! Lucro Target: 200€/dia

use alloy::primitives::U256;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{info, warn, debug};

/// 💰 CONSTANTES DE LUCRO (em USD)
pub const MIN_PROFIT_PER_TRADE: f64 = 2.00;      // Mínimo para executar
pub const PROFIT_AGGRESSIVE: f64 = 20.00;       // Ativa gas agressivo
pub const PROFIT_EXTREME: f64 = 100.00;          // Ativa gas extremo

/// ⛽ CONSTANTES DE GAS (em wei)
pub const GAS_TIP_MIN: u64 = 1_000_000_000;           // 1 gwei
pub const GAS_TIP_AGGRESSIVE: u64 = 50_000_000_000;    // 50 gwei
pub const GAS_TIP_EXTREME: u64 = 200_000_000_000;      // 200 gwei

/// 🎯 TARGET DIÁRIO
pub const DAILY_TARGET_EUR: f64 = 200.0;
pub const DAILY_TARGET_USD: f64 = 216.0; // 200€ * 1.08

/// 🐺 APEX PREDATOR ECONOMIC ENGINE
pub struct ApexPredator {
    /// Lucro acumulado hoje (USD)
    daily_profit_usd: Arc<RwLock<f64>>,
    
    /// Histórico de trades do dia
    trade_history: Arc<RwLock<VecDeque<CompletedTrade>>>,
    
    /// Contador de oportunidades descartadas (lucro < $2)
    discarded_opportunities: Arc<RwLock<u64>>,
    
    /// Contador de oportunidades executadas
    executed_trades: Arc<RwLock<u64>>,
    
    /// Gas total gasto hoje (wei)
    total_gas_spent: Arc<RwLock<U256>>,
    
    /// Modo agressivo ativado
    aggressive_mode: Arc<RwLock<bool>>,
}

/// 📊 Trade Completado
#[derive(Clone, Debug)]
pub struct CompletedTrade {
    pub timestamp: Instant,
    pub profit_usd: f64,
    pub gas_cost_eth: f64,
    pub gas_tip_gwei: u64,
    pub success: bool,
    pub trade_type: TradeType,
}

#[derive(Clone, Debug)]
pub enum TradeType {
    BackrunWhale,
    CrossDexArb,
    TriangularArb,
    Sandwich,
}

/// 💰 Avaliação de Oportunidade
#[derive(Clone, Debug)]
pub struct OpportunityEvaluation {
    pub gross_profit_usd: f64,
    pub estimated_gas_cost_eth: f64,
    pub net_profit_usd: f64,
    pub recommended_gas_tip: U256,
    pub should_execute: bool,
    pub priority: ExecutionPriority,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionPriority {
    Discard,      // Lucro < $2
    Normal,       // $2 < Lucro < $20
    Aggressive,   // $20 < Lucro < $100
    Extreme,      // Lucro > $100
}

impl ApexPredator {
    pub fn new() -> Self {
        Self {
            daily_profit_usd: Arc::new(RwLock::new(0.0)),
            trade_history: Arc::new(RwLock::new(VecDeque::new())),
            discarded_opportunities: Arc::new(RwLock::new(0)),
            executed_trades: Arc::new(RwLock::new(0)),
            total_gas_spent: Arc::new(RwLock::new(U256::ZERO)),
            aggressive_mode: Arc::new(RwLock::new(false)),
        }
    }
    
    /// 🚀 Inicia o Apex Predator
    pub async fn spawn(self: Arc<Self>) {
        info!("═══════════════════════════════════════════════════════════");
        info!("🐺🐺🐺 APEX PREDATOR - Economic Engine");
        info!("═══════════════════════════════════════════════════════════");
        info!("💰 Min Profit: ${:.2} | Aggressive: ${:.2} | Extreme: ${:.2}", 
            MIN_PROFIT_PER_TRADE, PROFIT_AGGRESSIVE, PROFIT_EXTREME);
        info!("⛽ Gas Tips: {} / {} / {} gwei", 
            GAS_TIP_MIN / 1_000_000_000,
            GAS_TIP_AGGRESSIVE / 1_000_000_000,
            GAS_TIP_EXTREME / 1_000_000_000);
        info!("🎯 Daily Target: {:.2}€ (${:.2})", DAILY_TARGET_EUR, DAILY_TARGET_USD);
        info!("═══════════════════════════════════════════════════════════");
        
        // Spawn monitoring loop
        let predator = self.clone();
        tokio::spawn(async move {
            predator.monitoring_loop().await;
        });
    }
    
    /// 🔄 Loop de monitorização
    async fn monitoring_loop(&self) {
        let mut interval = interval(Duration::from_secs(10));
        
        loop {
            interval.tick().await;
            
            let profit = *self.daily_profit_usd.read().await;
            let discarded = *self.discarded_opportunities.read().await;
            let executed = *self.executed_trades.read().await;
            let gas_spent = *self.total_gas_spent.read().await;
            
            let progress = (profit / DAILY_TARGET_USD) * 100.0;
            let gas_eth = gas_spent.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
            
            info!("📊📊📊 [APEX STATUS] Profit: ${:.2} ({:.1}%) | Trades: {} | Discarded: {} | Gas: {:.4} ETH",
                profit, progress, executed, discarded, gas_eth);
            
            // Alerta quando atingir meta
            if profit >= DAILY_TARGET_USD {
                info!("🎉🎉🎉 [APEX] META DIÁRIA ATINGIDA! ${:.2} / {:.2}€", profit, DAILY_TARGET_EUR);
            }
        }
    }
    
    /// 🎯 Avalia oportunidade de arbitragem
    pub async fn evaluate_opportunity(
        &self,
        gross_profit_usd: f64,
        estimated_gas_used: u64,
        current_gas_price_gwei: f64,
    ) -> OpportunityEvaluation {
        let gas_cost_eth = (estimated_gas_used as f64 * current_gas_price_gwei * 1e9) / 1e18;
        let gas_cost_usd = gas_cost_eth * 2500.0; // ETH @ $2500
        
        let net_profit = gross_profit_usd - gas_cost_usd;
        
        // Filtro de lucro mínimo ($2)
        if net_profit < MIN_PROFIT_PER_TRADE {
            let mut discarded = self.discarded_opportunities.write().await;
            *discarded += 1;
            
            return OpportunityEvaluation {
                gross_profit_usd,
                estimated_gas_cost_eth: gas_cost_eth,
                net_profit_usd: net_profit,
                recommended_gas_tip: U256::ZERO,
                should_execute: false,
                priority: ExecutionPriority::Discard,
                reason: format!("Lucro ${:.2} < mínimo ${:.2}", net_profit, MIN_PROFIT_PER_TRADE),
            };
        }
        
        // Gas War Control - Calcular tip baseado no lucro
        let (priority, gas_tip, reason) = self.calculate_gas_strategy(net_profit);
        
        debug!("💰 [APEX] Oportunidade aprovada! Lucro: ${:.2} | Gas: {:.4} ETH | Strategy: {:?}",
            net_profit, gas_cost_eth, priority);
        
        OpportunityEvaluation {
            gross_profit_usd,
            estimated_gas_cost_eth: gas_cost_eth,
            net_profit_usd: net_profit,
            recommended_gas_tip: U256::from(gas_tip),
            should_execute: true,
            priority,
            reason,
        }
    }
    
    /// ⛽ GAS WAR CONTROL - Estratégia de Gas
    fn calculate_gas_strategy(&self, net_profit: f64) -> (ExecutionPriority, u64, String) {
        if net_profit >= PROFIT_EXTREME {
            // 🔥🔥🔥 Lucro > $100: GAS EXTREMO (200 gwei)
            (
                ExecutionPriority::Extreme,
                GAS_TIP_EXTREME,
                format!("🔥🔥🔥 GAS EXTREMO ativado! Lucro ${:.2} > ${:.2}. Vamos ganhar esta guerra!", 
                    net_profit, PROFIT_EXTREME)
            )
        } else if net_profit >= PROFIT_AGGRESSIVE {
            // 🔥 Lucro > $20: GAS AGRESSIVO (50 gwei)
            (
                ExecutionPriority::Aggressive,
                GAS_TIP_AGGRESSIVE,
                format!("🔥 GAS AGRESSIVO ativado! Lucro ${:.2} > ${:.2}", 
                    net_profit, PROFIT_AGGRESSIVE)
            )
        } else {
            // Lucro > $2: GAS MÍNIMO (1 gwei)
            (
                ExecutionPriority::Normal,
                GAS_TIP_MIN,
                format!("Gas normal. Lucro ${:.2} acima do mínimo ${:.2}", 
                    net_profit, MIN_PROFIT_PER_TRADE)
            )
        }
    }
    
    /// ✅ Registra trade executado com sucesso
    pub async fn record_successful_trade(
        &self,
        profit_usd: f64,
        gas_cost_eth: f64,
        gas_tip_gwei: u64,
        trade_type: TradeType,
    ) {
        let trade = CompletedTrade {
            timestamp: Instant::now(),
            profit_usd,
            gas_cost_eth,
            gas_tip_gwei,
            success: true,
            trade_type: trade_type.clone(),
        };
        
        let mut history = self.trade_history.write().await;
        history.push_back(trade);
        
        // Manter só últimas 1000 trades
        if history.len() > 1000 {
            history.pop_front();
        }
        drop(history);
        
        // Atualizar lucro acumulado
        let mut profit = self.daily_profit_usd.write().await;
        *profit += profit_usd;
        drop(profit);
        
        // Atualizar contadores
        let mut executed = self.executed_trades.write().await;
        *executed += 1;
        
        let mut gas = self.total_gas_spent.write().await;
        *gas += U256::from((gas_cost_eth * 1e18) as u128);
        
        info!("✅✅✅ [APEX TRADE] Sucesso! +${:.2} | Gas: {} gwei | Type: {:?}",
            profit_usd, gas_tip_gwei, trade_type.clone());
    }
    
    /// ❌ Registra trade falhado
    pub async fn record_failed_trade(
        &self,
        gas_cost_eth: f64,
        gas_tip_gwei: u64,
        trade_type: TradeType,
        reason: String,
    ) {
        let trade = CompletedTrade {
            timestamp: Instant::now(),
            profit_usd: 0.0,
            gas_cost_eth,
            gas_tip_gwei,
            success: false,
            trade_type,
        };
        
        let mut history = self.trade_history.write().await;
        history.push_back(trade);
        drop(history);
        
        let mut gas = self.total_gas_spent.write().await;
        *gas += U256::from((gas_cost_eth * 1e18) as u128);
        
        warn!("❌ [APEX TRADE] Falhou! Gas perdido: {:.4} ETH | Reason: {}", 
            gas_cost_eth, reason);
    }
    
    /// 📊 Retorna estatísticas do dia
    pub async fn get_daily_stats(&self) -> DailyStats {
        let profit = *self.daily_profit_usd.read().await;
        let discarded = *self.discarded_opportunities.read().await;
        let executed = *self.executed_trades.read().await;
        let gas_spent = *self.total_gas_spent.read().await;
        
        let trades = self.trade_history.read().await;
        let successful_trades: Vec<_> = trades.iter().filter(|t| t.success).collect();
        let total_trades = trades.len();
        
        let success_rate = if total_trades > 0 {
            (successful_trades.len() as f64 / total_trades as f64) * 100.0
        } else {
            0.0
        };
        
        let avg_gas_tip = if total_trades > 0 {
            trades.iter().map(|t| t.gas_tip_gwei).sum::<u64>() as f64 / total_trades as f64
        } else {
            0.0
        };
        
        DailyStats {
            profit_usd: profit,
            profit_eur: profit / 1.08,
            target_progress: (profit / DAILY_TARGET_USD) * 100.0,
            executed_trades: executed,
            discarded_opportunities: discarded,
            success_rate_percent: success_rate,
            total_gas_spent_eth: gas_spent.try_into().unwrap_or(u128::MAX) as f64 / 1e18,
            average_gas_tip_gwei: avg_gas_tip,
        }
    }
    
    /// 🔄 Reseta lucro diário (chamar à meia-noite)
    pub async fn reset_daily_stats(&self) {
        let mut profit = self.daily_profit_usd.write().await;
        *profit = 0.0;
        drop(profit);
        
        let mut executed = self.executed_trades.write().await;
        *executed = 0;
        drop(executed);
        
        let mut discarded = self.discarded_opportunities.write().await;
        *discarded = 0;
        drop(discarded);
        
        let mut gas = self.total_gas_spent.write().await;
        *gas = U256::ZERO;
        drop(gas);
        
        let mut history = self.trade_history.write().await;
        history.clear();
        drop(history);
        
        info!("🔄 [APEX] Estatísticas diárias resetadas");
    }
    
    /// 🎯 Verifica se está próximo de atingir meta
    pub async fn is_near_target(&self, threshold_percent: f64) -> bool {
        let profit = *self.daily_profit_usd.read().await;
        let progress = (profit / DAILY_TARGET_USD) * 100.0;
        progress >= (100.0 - threshold_percent)
    }
}

/// 📊 Estatísticas Diárias
#[derive(Clone, Debug)]
pub struct DailyStats {
    pub profit_usd: f64,
    pub profit_eur: f64,
    pub target_progress: f64,
    pub executed_trades: u64,
    pub discarded_opportunities: u64,
    pub success_rate_percent: f64,
    pub total_gas_spent_eth: f64,
    pub average_gas_tip_gwei: f64,
}

impl Default for ApexPredator {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ApexPredator {
    fn clone(&self) -> Self {
        Self {
            daily_profit_usd: self.daily_profit_usd.clone(),
            trade_history: self.trade_history.clone(),
            discarded_opportunities: self.discarded_opportunities.clone(),
            executed_trades: self.executed_trades.clone(),
            total_gas_spent: self.total_gas_spent.clone(),
            aggressive_mode: self.aggressive_mode.clone(),
        }
    }
}
