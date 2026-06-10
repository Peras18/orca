//! 💸 Liquidation Monitor — Aave/Moonwell liquidation opportunities
//!
//! Moonwell BASE: 0xfBb21d0380beE3312B33c4353c8936a0F13EF26E

use alloy::primitives::{Address, U256};
use tracing::info;

/// Endereços de protocolos de lending na BASE
pub const MOONWELL_COMPTROLLER: &str = "0xfBb21d0380beE3312B33c4353c8936a0F13EF26E";
pub const AAVE_POOL_BASE: &str = "0xA238Dd80C2594B40d7b3f6bAd1F36c2bcEfaD409";

/// Monitor de liquidações
pub struct LiquidationMonitor {
    min_premium_bps: u32,     // Mínimo de prémio (ex: 10500 = 5%)
    gas_cost_threshold: U256, // Máximo de gas a pagar
}

impl LiquidationMonitor {
    pub fn new() -> Self {
        Self {
            min_premium_bps: 10500, // 5% premium
            gas_cost_threshold: U256::from(500_000), // 500k gas
        }
    }

    /// Verifica se uma posição é liquidável com lucro
    pub fn check_liquidation_profit(
        &self,
        debt_asset: Address,
        collateral_asset: Address,
        debt_amount: U256,
        collateral_amount: U256,
        gas_price: U256,
    ) -> Option<LiquidationOpportunity> {
        // Prémio de liquidação (ex: 5%)
        let premium = collateral_amount * U256::from(self.min_premium_bps) / U256::from(10000);
        let gross_profit = premium.saturating_sub(debt_amount);
        
        // Custo de gas
        let gas_cost = gas_price * self.gas_cost_threshold;
        
        if gross_profit > gas_cost {
            Some(LiquidationOpportunity {
                debt_asset,
                collateral_asset,
                debt_amount,
                collateral_amount,
                expected_profit: gross_profit - gas_cost,
                gas_cost,
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct LiquidationOpportunity {
    pub debt_asset: Address,
    pub collateral_asset: Address,
    pub debt_amount: U256,
    pub collateral_amount: U256,
    pub expected_profit: U256,
    pub gas_cost: U256,
}
