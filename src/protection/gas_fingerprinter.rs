//! ⛽ GasFingerprinter — Randomização de gas para evitar deteção
//!
//! Os bundles MEV são identificáveis por padrões fixos de gas price.
//! Este módulo adiciona jitter aleatório ao gas price e priority fee
//! para misturar transações com tráfego normal da rede.

use alloy::primitives::U256;
use rand::Rng;
use tracing::debug;

/// Ranges de jitter para diferentes estratégias de camuflagem
pub struct GasFingerprintConfig {
    /// Jitter máximo em % do gas base (ex: 5 = ±5%)
    pub base_jitter_bps: u64,
    /// Jitter máximo da priority fee em %
    pub priority_jitter_bps: u64,
    /// Probabilidade de simular um "usuário normal" (não MEV)
    pub normal_user_chance: f64,
    /// Gas price típico de usuário normal (para simulação)
    pub normal_gas_gwei: u64,
}

impl Default for GasFingerprintConfig {
    fn default() -> Self {
        Self {
            base_jitter_bps: 500,      // ±5%
            priority_jitter_bps: 1000, // ±10%
            normal_user_chance: 0.15,    // 15% de chance de parecer "normal"
            normal_gas_gwei: 2,        // 2 gwei (padrão Base)
        }
    }
}

/// Randomiza gas price e priority fee para evitar fingerprinting
pub struct GasFingerprinter {
    config: GasFingerprintConfig,
}

impl GasFingerprinter {
    pub fn new(config: GasFingerprintConfig) -> Self {
        Self { config }
    }

    /// Aplica fingerprint aleatório a um gas price
    pub fn apply(&self, base_gas_price_wei: U256, priority_fee_wei: U256) -> (U256, U256) {
        let mut rng = rand::thread_rng();

        // Estratégia 1: Camuflagem como usuário normal (ocasional)
        if rng.gen::<f64>() < self.config.normal_user_chance {
            let normal_gas = U256::from(self.config.normal_gas_gwei) * U256::from(1_000_000_000u64);
            let jitter = rng.gen_range(95..=105); // ±5%
            let gas = normal_gas * U256::from(jitter) / U256::from(100);
            debug!("⛽ GasFingerprinter: modo 'usuário normal' — {} gwei", gas);
            return (gas, U256::ZERO);
        }

        // Estratégia 2: Jitter aleatório no gas base (±base_jitter_bps)
        let base_jitter = rng.gen_range(
            10000 - self.config.base_jitter_bps..=10000 + self.config.base_jitter_bps,
        );
        let randomized_gas = base_gas_price_wei * U256::from(base_jitter) / U256::from(10000);

        // Estratégia 3: Jitter na priority fee (±priority_jitter_bps)
        let priority_jitter = rng.gen_range(
            10000 - self.config.priority_jitter_bps..=10000 + self.config.priority_jitter_bps,
        );
        let randomized_priority = priority_fee_wei * U256::from(priority_jitter) / U256::from(10000);

        debug!(
            "⛽ GasFingerprinter: gas={}wei priority={}wei (jitter: {}bps / {}bps)",
            randomized_gas, randomized_priority,
            base_jitter, priority_jitter
        );

        (randomized_gas, randomized_priority)
    }

    /// Versão agressiva para leilões de MEV — jitter mínimo para garantir win
    pub fn apply_aggressive(&self, base_gas_price_wei: U256, min_priority_wei: U256) -> (U256, U256) {
        let mut rng = rand::thread_rng();

        // Apenas jitter positivo pequeno para garantir vantagem
        let priority_jitter = rng.gen_range(10000..=10200); // +0% a +2%
        let randomized_priority = min_priority_wei * U256::from(priority_jitter) / U256::from(10000);

        (base_gas_price_wei, randomized_priority)
    }
}
