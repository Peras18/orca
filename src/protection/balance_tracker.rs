//! 💰 Balance Tracker — Rastreamento de lucro/perda por estratégia
use alloy::primitives::U256;
use std::collections::HashMap;
use parking_lot::RwLock;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyMetrics {
    pub strategy: String,
    pub executions: u64,
    pub wins: u64,
    pub losses: u64,
    pub total_profit_wei: U256,
    pub total_loss_wei: U256,
    pub avg_gas_cost: u64,
}

pub struct BalanceTracker {
    by_strategy: RwLock<HashMap<String, StrategyMetrics>>,
    by_pool: RwLock<HashMap<String, (u64, U256)>>, // (executions, profit)
    session_start_balance: RwLock<U256>,
    session_max_loss_bps: u32, // 10% = 1000 bps
}

impl BalanceTracker {
    pub fn new(initial_balance: U256) -> Self {
        Self {
            by_strategy: RwLock::new(HashMap::new()),
            by_pool: RwLock::new(HashMap::new()),
            session_start_balance: RwLock::new(initial_balance),
            session_max_loss_bps: 1000, // 10%
        }
    }

    pub fn record_execution(&self, strategy: &str, pool: &str, profit: U256, gas: u64) {
        let mut by_strat = self.by_strategy.write();
        let entry = by_strat.entry(strategy.to_string()).or_insert(StrategyMetrics {
            strategy: strategy.to_string(),
            executions: 0,
            wins: 0,
            losses: 0,
            total_profit_wei: U256::ZERO,
            total_loss_wei: U256::ZERO,
            avg_gas_cost: 0,
        });
        
        entry.executions += 1;
        entry.avg_gas_cost = (entry.avg_gas_cost * (entry.executions - 1) + gas) / entry.executions;
        
        if profit > U256::ZERO {
            entry.wins += 1;
            entry.total_profit_wei += profit;
        } else {
            entry.losses += 1;
            entry.total_loss_wei += profit;
        }

        let mut by_pool_map = self.by_pool.write();
        let (execs, total) = by_pool_map.entry(pool.to_string()).or_insert((0, U256::ZERO));
        *execs += 1;
        *total += profit;
    }

    pub fn check_session_loss(&self, current_balance: U256) -> bool {
        let start = *self.session_start_balance.read();
        if current_balance >= start {
            return false;
        }
        
        let loss = start - current_balance;
        let loss_bps = (loss * U256::from(10000)) / start;
        
        loss_bps >= U256::from(self.session_max_loss_bps)
    }

    pub fn best_strategies(&self) -> Vec<StrategyMetrics> {
        let by_strat = self.by_strategy.read();
        let mut metrics: Vec<_> = by_strat.values().cloned().collect();
        metrics.sort_by(|a, b| b.total_profit_wei.cmp(&a.total_profit_wei));
        metrics
    }
}
