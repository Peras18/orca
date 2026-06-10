//! ⛽ Gas Oracle — Percentil 10 de priority fee
use alloy::primitives::U256;
use std::collections::VecDeque;
use parking_lot::RwLock;
use tracing::info;

/// Histórico de fees por bloco
pub struct GasOracle {
    /// Priority fees dos últimos blocos (em wei)
    history: RwLock<VecDeque<u128>>,
    /// Tamanho máximo do histórico (20 blocos)
    max_history: usize,
    /// Fee mínimo para inclusão (percentil 10)
    current_fee: RwLock<u128>,
    /// Multiplicador de profit-to-gas (mínimo 3.0)
    min_profit_ratio: f64,
}

impl GasOracle {
    pub fn new() -> Self {
        Self {
            history: RwLock::new(VecDeque::with_capacity(20)),
            max_history: 20,
            current_fee: RwLock::new(1_000_000_000), // 1 gwei default
            min_profit_ratio: 3.0,
        }
    }

    /// Atualiza com novo bloco
    pub fn update(&self, priority_fee: u128) {
        let mut hist = self.history.write();
        hist.push_back(priority_fee);
        if hist.len() > self.max_history {
            hist.pop_front();
        }

        // Calcular percentil 10
        if hist.len() >= 5 {
            let mut sorted: Vec<_> = hist.iter().copied().collect();
            sorted.sort_unstable();
            let p10_idx = (sorted.len() as f64 * 0.1) as usize;
            let p10 = sorted[p10_idx.max(0).min(sorted.len() - 1)];
            
            *self.current_fee.write() = p10;
        }
    }

    /// Fee recomendada para inclusão rápida (percentil 10)
    pub fn recommended_fee(&self) -> u128 {
        *self.current_fee.read()
    }

    /// Verifica se profit cobre gas com margem 3:1
    pub fn is_profitable(&self, gross_profit_wei: U256, gas_estimate: u64) -> bool {
        let fee = self.recommended_fee();
        let gas_cost = U256::from(gas_estimate) * U256::from(fee);
        
        if gas_cost.is_zero() {
            return true;
        }
        
        let ratio = gross_profit_wei.to_string().parse::<f64>().unwrap_or(0.0)
            / gas_cost.to_string().parse::<f64>().unwrap_or(1.0);
        
        ratio >= self.min_profit_ratio
    }

    /// Estima custo total de gas
    pub fn estimate_cost(&self, gas_units: u64) -> U256 {
        U256::from(gas_units) * U256::from(self.recommended_fee())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_oracle_p10() {
        let oracle = GasOracle::new();
        
        // Adicionar 20 valores ordenados
        for i in 1..=20 {
            oracle.update(i as u128 * 1_000_000_000);
        }
        
        // Percentil 10 de 1..20 = ~2
        let fee = oracle.recommended_fee();
        assert!(fee >= 2_000_000_000 && fee <= 4_000_000_000);
    }
}
