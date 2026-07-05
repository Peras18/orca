//! 💀 Liquidation Hunter — antecipa liquidações em lending protocols
//! Monitoriza posições em Moonwell e Aave V3 na Base
//! Liquida antes do oráculo atualizar oficialmente

use alloy::primitives::{Address, U256};
use std::collections::HashMap;
use tracing::{info, warn};

/// Posição de empréstimo monitorizada
#[derive(Debug, Clone)]
pub struct BorrowPosition {
    pub user: Address,
    pub protocol: LendingProtocol,
    pub collateral_token: Address,
    pub debt_token: Address,
    pub collateral_amount: U256,
    pub debt_amount: U256,
    /// Health factor atual (1.0 = limiar de liquidação)
    pub health_factor: f64,
    /// Bloco da última atualização
    pub last_update: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LendingProtocol {
    MoonwellBase,
    AaveV3Base,
    CompoundV3Base,
}

impl LendingProtocol {
    pub fn liquidation_bonus_bps(&self) -> u32 {
        match self {
            Self::MoonwellBase => 800,   // 8% bonus
            Self::AaveV3Base => 500,     // 5% bonus
            Self::CompoundV3Base => 600, // 6% bonus
        }
    }

    pub fn contract_address(&self) -> Address {
        use alloy::primitives::address;
        match self {
            // Moonwell Comptroller na Base
            Self::MoonwellBase => address!("0xfBb21d0380beE3312B33c4353c8936a0F13EF26C"),
            // Aave V3 Pool na Base
            Self::AaveV3Base => address!("0xA238Dd8c259C4b2e4b1529fAf70dC6DA397Ba70a"),
            // Compound V3 na Base
            Self::CompoundV3Base => address!("0xb125E6687d4313864e53df431d5425969c15Eb20"),
        }
    }
}

/// Motor de caça a liquidações
#[derive(Debug)]
pub struct LiquidationHunter {
    /// Posições monitorizadas
    positions: HashMap<Address, BorrowPosition>,
    /// Threshold de health factor para alertar (ex: 1.05 = 5% acima do limiar)
    alert_threshold: f64,
    /// Preços atuais dos tokens (em USD com 8 decimais)
    token_prices: HashMap<Address, f64>,
    /// Contador de liquidações simuladas
    pub simulated_liquidations: u64,
    /// Lucro total simulado em USD
    pub simulated_profit_usd: f64,
}

impl LiquidationHunter {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            alert_threshold: 1.05, // alerta quando HF < 1.05
            token_prices: HashMap::new(),
            simulated_liquidations: 0,
            simulated_profit_usd: 0.0,
        }
    }

    /// Atualiza preço de um token
    pub fn update_price(&mut self, token: Address, price_usd: f64) {
        self.token_prices.insert(token, price_usd);
    }

    /// Regista ou atualiza posição de empréstimo
    pub fn track_position(&mut self, pos: BorrowPosition) {
        if pos.health_factor < self.alert_threshold {
            warn!(
                "[LIQUIDATION] ⚠️ Posição em risco | User: {:?} | HF: {:.4} | Protocol: {:?}",
                pos.user, pos.health_factor, pos.protocol
            );
        }
        self.positions.insert(pos.user, pos);
    }

    /// Verifica todas as posições e retorna as liquidáveis
    pub fn get_liquidatable(&self) -> Vec<&BorrowPosition> {
        self.positions.values()
            .filter(|p| p.health_factor < 1.0)
            .collect()
    }

    /// Calcula lucro estimado de liquidar uma posição
    /// Lucro = collateral_seized * bonus_pct - debt_repaid - gas
    pub fn estimate_liquidation_profit(
        &self,
        pos: &BorrowPosition,
        gas_cost_usd: f64,
    ) -> Option<f64> {
        let collateral_price = self.token_prices.get(&pos.collateral_token)?;
        let debt_price = self.token_prices.get(&pos.debt_token)?;

        // Máximo liquidável: 50% da dívida (Aave/Moonwell limit)
        let debt_to_repay_usd = (pos.debt_amount.try_into().unwrap_or(u128::MAX) as f64 / 1e18)
            * debt_price * 0.5;

        let bonus = pos.protocol.liquidation_bonus_bps() as f64 / 10_000.0;
        let collateral_seized_usd = debt_to_repay_usd * (1.0 + bonus);

        let gross_profit = collateral_seized_usd - debt_to_repay_usd;
        let net_profit = gross_profit - gas_cost_usd;

        if net_profit > 0.0 {
            info!(
                "[LIQUIDATION] 💰 Oportunidade | User: {:?} | Gross: ${:.2} | Net: ${:.2}",
                pos.user, gross_profit, net_profit
            );
            Some(net_profit)
        } else {
            None
        }
    }

    /// Simula liquidação para DRY_RUN
    pub fn simulate_liquidation(&mut self, pos: &BorrowPosition, profit_usd: f64) {
        self.simulated_liquidations += 1;
        self.simulated_profit_usd += profit_usd;
        info!(
            "[DRY_RUN] Liquidação simulada #{} | Profit: ${:.2} | Total: ${:.2}",
            self.simulated_liquidations,
            profit_usd,
            self.simulated_profit_usd
        );
    }

    /// Health factor baseado nos preços atuais
    pub fn recalculate_health_factor(&self, pos: &BorrowPosition) -> Option<f64> {
        let collateral_price = self.token_prices.get(&pos.collateral_token)?;
        let debt_price = self.token_prices.get(&pos.debt_token)?;

        // Liquidation threshold típico: 80% (varia por protocolo)
        let liquidation_threshold = 0.80;

        let collateral_usd = (pos.collateral_amount.try_into().unwrap_or(u128::MAX) as f64 / 1e18)
            * collateral_price * liquidation_threshold;
        let debt_usd = (pos.debt_amount.try_into().unwrap_or(u128::MAX) as f64 / 1e18) * debt_price;

        if debt_usd == 0.0 { return Some(f64::MAX); }
        Some(collateral_usd / debt_usd)
    }

    pub fn position_count(&self) -> usize {
        self.positions.len()
    }

    pub fn at_risk_count(&self) -> usize {
        self.positions.values()
            .filter(|p| p.health_factor < self.alert_threshold)
            .count()
    }
}
