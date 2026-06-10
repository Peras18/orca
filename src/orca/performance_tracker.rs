//! ORCA PERFORMANCE TRACKER
//! 
//! Logs de performance:
//! [ORCA-HIT] Lucro líquido real extraído
//! [GAS-SAVED] Economia gerada pelo código Yul
//! [BANK-TOTAL] Saldo acumulado com juros compostos

use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 📊 Tracker de Performance
#[derive(Clone, Debug)]
pub struct PerformanceTracker {
    /// Lucro total extraído (ETH)
    total_profit_eth: Arc<RwLock<f64>>,
    /// Lucro hoje (ETH)
    daily_profit_eth: Arc<RwLock<f64>>,
    /// Gás economizado total (ETH)
    total_gas_saved_eth: Arc<RwLock<f64>>,
    /// Gás economizado hoje (ETH)
    daily_gas_saved_eth: Arc<RwLock<f64>>,
    /// Saldo acumulado (ETH) - com juros compostos
    bank_total_eth: Arc<RwLock<f64>>,
    /// Capital inicial (ETH)
    initial_capital_eth: Arc<RwLock<f64>>,
    /// Contador de hits
    hit_count: Arc<RwLock<u64>>,
    /// Contador de hits hoje
    daily_hits: Arc<RwLock<u64>>,
    /// Timestamp de início do dia
    day_start: Arc<RwLock<u64>>,
    /// Histórico de lucros
    profit_history: Arc<RwLock<Vec<ProfitLog>>>,
    /// Histórico de gás economizado
    gas_history: Arc<RwLock<Vec<GasLog>>>,
    /// ROI acumulado (%)
    roi_pct: Arc<RwLock<f64>>,
}

/// 💰 Log de Lucro
#[derive(Clone, Debug)]
pub struct ProfitLog {
    /// Timestamp
    pub timestamp: u64,
    /// Lucro bruto (ETH)
    pub gross_profit_eth: f64,
    /// Lucro líquido (ETH)
    pub net_profit_eth: f64,
    /// Custo de gás (ETH)
    pub gas_cost_eth: f64,
    /// Tipo de oportunidade
    pub opportunity_type: String,
    /// Block number
    pub block_number: u64,
    /// Slot no bloco
    pub block_slot: u16,
    /// TX hash
    pub tx_hash: String,
}

/// ⛽ Log de Gás Economizado
#[derive(Clone, Debug)]
pub struct GasLog {
    /// Timestamp
    pub timestamp: u64,
    /// Gás padrão que seria usado
    pub baseline_gas: u64,
    /// Gás real usado (Yul)
    pub optimized_gas: u64,
    /// Gás economizado
    pub gas_saved: u64,
    /// Economia em ETH
    pub savings_eth: f64,
    /// Porcentagem de economia
    pub savings_pct: f64,
}

/// 💎 Log de Saldo (Bank Total)
#[derive(Clone, Debug)]
pub struct BankLog {
    /// Timestamp
    pub timestamp: u64,
    /// Saldo anterior
    pub previous_balance: f64,
    /// Saldo atual
    pub current_balance: f64,
    /// Variação
    pub change_eth: f64,
    /// ROI desde início (%)
    pub roi_pct: f64,
    /// APR estimado (%)
    pub apr_pct: f64,
}

/// 📈 Estatísticas Diárias
#[derive(Clone, Debug)]
pub struct DailyStats {
    /// Data
    pub date: String,
    /// Lucro do dia (ETH)
    pub profit_eth: f64,
    /// Lucro em €
    pub profit_eur: f64,
    /// Número de trades
    pub trade_count: u64,
    /// Taxa de sucesso (%)
    pub success_rate: f64,
    /// Gás economizado (ETH)
    pub gas_saved_eth: f64,
    /// ROI do dia (%)
    pub daily_roi_pct: f64,
}

/// 🎯 Métricas de Performance
#[derive(Clone, Debug)]
pub struct PerformanceMetrics {
    /// Lucro médio por trade (ETH)
    pub avg_profit_per_trade: f64,
    /// Lucro médio por trade (€)
    pub avg_profit_per_trade_eur: f64,
    /// Melhor trade (ETH)
    pub best_trade_eth: f64,
    /// Pior trade (ETH)
    pub worst_trade_eth: f64,
    /// Gás médio economizado por trade (%)
    pub avg_gas_savings_pct: f64,
    /// Tempo médio entre trades (min)
    pub avg_time_between_trades_min: f64,
}

impl PerformanceTracker {
    /// 🚀 Inicializa tracker
    pub fn new() -> Self {
        info!("═══════════════════════════════════════════════════════════");
        info!("📊 ORCA PERFORMANCE TRACKER");
        info!("[ORCA-HIT] Lucro líquido real extraído");
        info!("[GAS-SAVED] Economia Yul Assembly");
        info!("[BANK-TOTAL] Saldo com juros compostos");
        info!("═══════════════════════════════════════════════════════════");
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        Self {
            total_profit_eth: Arc::new(RwLock::new(0.0)),
            daily_profit_eth: Arc::new(RwLock::new(0.0)),
            total_gas_saved_eth: Arc::new(RwLock::new(0.0)),
            daily_gas_saved_eth: Arc::new(RwLock::new(0.0)),
            bank_total_eth: Arc::new(RwLock::new(0.0)),
            initial_capital_eth: Arc::new(RwLock::new(0.05)), // 0.05 ETH default
            hit_count: Arc::new(RwLock::new(0)),
            daily_hits: Arc::new(RwLock::new(0)),
            day_start: Arc::new(RwLock::new(now)),
            profit_history: Arc::new(RwLock::new(Vec::new())),
            gas_history: Arc::new(RwLock::new(Vec::new())),
            roi_pct: Arc::new(RwLock::new(0.0)),
        }
    }
    
    /// 🎯 Registra hit (execução com sucesso)
    pub async fn record_hit(
        &self,
        profit_log: ProfitLog,
        gas_log: GasLog,
    ) {
        // Verificar se é novo dia
        self.check_new_day().await;
        
        // Atualizar lucros
        let mut total_profit = self.total_profit_eth.write().await;
        *total_profit += profit_log.net_profit_eth;
        drop(total_profit);
        
        let mut daily_profit = self.daily_profit_eth.write().await;
        *daily_profit += profit_log.net_profit_eth;
        drop(daily_profit);
        
        // Atualizar gás economizado
        let mut total_gas = self.total_gas_saved_eth.write().await;
        *total_gas += gas_log.savings_eth;
        drop(total_gas);
        
        let mut daily_gas = self.daily_gas_saved_eth.write().await;
        *daily_gas += gas_log.savings_eth;
        drop(daily_gas);
        
        // Atualizar bank total
        let mut bank = self.bank_total_eth.write().await;
        let previous = *bank;
        *bank += profit_log.net_profit_eth;
        let current = *bank;
        drop(bank);
        
        // Atualizar contadores
        *self.hit_count.write().await += 1;
        *self.daily_hits.write().await += 1;
        
        // Calcular ROI
        let initial = *self.initial_capital_eth.read().await;
        let roi = if initial > 0.0 {
            (current - initial) / initial * 100.0
        } else {
            0.0
        };
        *self.roi_pct.write().await = roi;
        
        // Guardar nos históricos
        self.profit_history.write().await.push(profit_log.clone());
        self.gas_history.write().await.push(gas_log.clone());
        
        // LOGS PRINCIPAIS
        info!(
            "[ORCA-HIT] 💰 Lucro: {} ETH | Tipo: {} | Block: {} | Slot: {} | TX: {}...",
            profit_log.net_profit_eth,
            profit_log.opportunity_type,
            profit_log.block_number,
            profit_log.block_slot,
            &profit_log.tx_hash[..20]
        );
        
        info!(
            "[GAS-SAVED] ⛽ Economia Yul: {} ETH | Gas: {} → {} | {:.1}% poupado",
            gas_log.savings_eth,
            gas_log.baseline_gas,
            gas_log.optimized_gas,
            gas_log.savings_pct
        );
        
        info!(
            "[BANK-TOTAL] 💎 Saldo: {} ETH | Variação: {:+} ETH | ROI: {:.2}% | APR: {:.1}%",
            current,
            current - previous,
            roi,
            self.calculate_apr().await
        );
    }
    
    /// 🔄 Verifica se é novo dia e reseta contadores
    async fn check_new_day(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let day_start = *self.day_start.read().await;
        
        if now - day_start >= 86400 { // 24 horas
            // Novo dia - log do dia anterior
            let daily_profit = *self.daily_profit_eth.read().await;
            let daily_hits = *self.daily_hits.read().await;
            let daily_gas = *self.daily_gas_saved_eth.read().await;
            
            info!("═══════════════════════════════════════════════════════════");
            info!("📅 RESUMO DO DIA ANTERIOR");
            info!("💰 Lucro: {} ETH | {} execuções", daily_profit, daily_hits);
            info!("⛽ Gás economizado: {} ETH", daily_gas);
            info!("═══════════════════════════════════════════════════════════");
            
            // Reset diário
            *self.daily_profit_eth.write().await = 0.0;
            *self.daily_gas_saved_eth.write().await = 0.0;
            *self.daily_hits.write().await = 0;
            *self.day_start.write().await = now;
        }
    }
    
    /// 📈 Calcula APR estimado
    async fn calculate_apr(&self) -> f64 {
        let profit = *self.total_profit_eth.read().await;
        let initial = *self.initial_capital_eth.read().await;
        let day_start = *self.day_start.read().await;
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let days_running = (now - day_start) as f64 / 86400.0;
        
        if days_running < 0.01 || initial == 0.0 {
            return 0.0;
        }
        
        let daily_return = profit / initial / days_running;
        daily_return * 365.0 * 100.0 // APR em %
    }
    
    /// 📊 Retorna estatísticas diárias
    pub async fn daily_stats(&self) -> DailyStats {
        self.check_new_day().await;
        
        let profit = *self.daily_profit_eth.read().await;
        let hits = *self.daily_hits.read().await;
        let gas_saved = *self.daily_gas_saved_eth.read().await;
        let initial = *self.initial_capital_eth.read().await;
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let date = chrono::DateTime::from_timestamp(now as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        
        let eth_eur_rate = 1600.0; // ~1600 EUR/ETH
        
        DailyStats {
            date,
            profit_eth: profit,
            profit_eur: profit * eth_eur_rate,
            trade_count: hits,
            success_rate: 100.0, // Simplificado
            gas_saved_eth: gas_saved,
            daily_roi_pct: if initial > 0.0 { profit / initial * 100.0 } else { 0.0 },
        }
    }
    
    /// 🎯 Retorna métricas de performance
    pub async fn metrics(&self) -> PerformanceMetrics {
        let history = self.profit_history.read().await;
        let gas_history = self.gas_history.read().await;
        let hits = *self.hit_count.read().await;
        
        if hits == 0 {
            return PerformanceMetrics {
                avg_profit_per_trade: 0.0,
                avg_profit_per_trade_eur: 0.0,
                best_trade_eth: 0.0,
                worst_trade_eth: 0.0,
                avg_gas_savings_pct: 0.0,
                avg_time_between_trades_min: 0.0,
            };
        }
        
        let profits: Vec<f64> = history.iter().map(|h| h.net_profit_eth).collect();
        let best = profits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let worst = profits.iter().cloned().fold(f64::INFINITY, f64::min);
        let avg_profit = profits.iter().sum::<f64>() / hits as f64;
        
        let avg_gas_savings = if !gas_history.is_empty() {
            gas_history.iter().map(|g| g.savings_pct).sum::<f64>() / gas_history.len() as f64
        } else {
            0.0
        };
        
        let eth_eur_rate = 1600.0;
        
        PerformanceMetrics {
            avg_profit_per_trade: avg_profit,
            avg_profit_per_trade_eur: avg_profit * eth_eur_rate,
            best_trade_eth: best,
            worst_trade_eth: worst,
            avg_gas_savings_pct: avg_gas_savings,
            avg_time_between_trades_min: 0.0, // Simplificado
        }
    }
    
    /// 📊 Estatísticas gerais
    pub async fn stats(&self) -> String {
        let total_profit = *self.total_profit_eth.read().await;
        let total_gas = *self.total_gas_saved_eth.read().await;
        let bank = *self.bank_total_eth.read().await;
        let hits = *self.hit_count.read().await;
        let roi = *self.roi_pct.read().await;
        let apr = self.calculate_apr().await;
        
        let eth_eur_rate = 1600.0;
        
        format!(
            "📊 ORCA | Hits: {} | Lucro: {} ETH ({}€) | Gás poupado: {} ETH | Saldo: {} ETH | ROI: {:.2}% | APR: {:.1}%",
            hits,
            total_profit,
            total_profit * eth_eur_rate,
            total_gas,
            bank,
            roi,
            apr
        )
    }
}

use tracing::info;
use chrono;
