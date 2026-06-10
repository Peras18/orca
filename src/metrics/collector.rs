//! 📊 Metrics Collector — Auto-optimização baseada em dados

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use parking_lot::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HourlyMetrics {
    pub timestamp: u64,
    pub opportunities_detected: u64,
    pub opportunities_executed: u64,
    pub opportunities_won: u64,
    pub profit_by_strategy: HashMap<String, u128>, // wei
    pub success_rate_bps: u32, // basis points (10000 = 100%)
    pub avg_gas_cost: u64,
    pub top_pools: Vec<(String, u64)>, // address, count
}

/// Coletor de métricas com auto-optimização
pub struct MetricsCollector {
    hourly: RwLock<Vec<HourlyMetrics>>,
    current_hour: RwLock<HourlyMetrics>,
    adaptive_min_profit: RwLock<U256>,
}

impl MetricsCollector {
    pub fn new(initial_min_profit: U256) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            hourly: RwLock::new(Vec::new()),
            current_hour: RwLock::new(HourlyMetrics {
                timestamp: now,
                opportunities_detected: 0,
                opportunities_executed: 0,
                opportunities_won: 0,
                profit_by_strategy: HashMap::new(),
                success_rate_bps: 0,
                avg_gas_cost: 0,
                top_pools: Vec::new(),
            }),
            adaptive_min_profit: RwLock::new(initial_min_profit),
        }
    }

    pub fn record_detection(&self) {
        self.current_hour.write().opportunities_detected += 1;
    }

    pub fn record_execution(&self, strategy: &str, profit: U256, gas: u64, success: bool) {
        let mut current = self.current_hour.write();
        current.opportunities_executed += 1;
        
        if success {
            current.opportunities_won += 1;
            let profit_u128 = profit.to::<u128>();
            *current.profit_by_strategy.entry(strategy.to_string()).or_insert(0) += profit_u128;
        }
        
        // Atualizar média de gas
        current.avg_gas_cost = (current.avg_gas_cost * (current.opportunities_executed - 1) + gas) 
            / current.opportunities_executed;
        
        // Recalcular taxa de sucesso
        if current.opportunities_executed > 0 {
            current.success_rate_bps = ((current.opportunities_won * 10000) / current.opportunities_executed) as u32;
        }
    }

    /// Ajusta min_profit baseado em performance
    pub fn adapt_min_profit(&self) {
        let current = self.current_hour.read();
        let rate = current.success_rate_bps;

        let mut min_profit = self.adaptive_min_profit.write();
        
        if rate < 6000 { // < 60%
            // Muito falhas, ser mais conservador
            *min_profit = *min_profit * U256::from(12) / U256::from(10); // +20%
            info!("📊 Adaptive: Increased min_profit by 20% (success: {}%)", rate / 100);
        } else if rate > 8000 { // > 80%
            // Muito sucesso, pode ser mais agressivo
            *min_profit = *min_profit * U256::from(9) / U256::from(10); // -10%
            info!("📊 Adaptive: Decreased min_profit by 10% (success: {}%)", rate / 100);
        }
    }

    /// Rotaciona métricas a cada hora
    pub fn rotate_hour(&self) {
        let mut hourly = self.hourly.write();
        let current = self.current_hour.read().clone();
        
        hourly.push(current);
        
        // Manter só 48 horas
        if hourly.len() > 48 {
            hourly.remove(0);
        }
        
        // Reset current
        *self.current_hour.write() = HourlyMetrics {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            opportunities_detected: 0,
            opportunities_executed: 0,
            opportunities_won: 0,
            profit_by_strategy: HashMap::new(),
            success_rate_bps: 0,
            avg_gas_cost: 0,
            top_pools: Vec::new(),
        };
        
        // Adaptar min_profit
        drop(hourly);
        self.adapt_min_profit();
    }

    pub fn export_json(&self) -> String {
        let hourly = self.hourly.read();
        serde_json::to_string_pretty(&*hourly).unwrap_or_default()
    }
}

use alloy::primitives::U256;
