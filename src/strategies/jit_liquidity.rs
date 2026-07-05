//! 🎯 JIT Liquidity — Just-In-Time em V3/Aerodrome CL
//!
//! Detecta swap grande pendente, adiciona liquidez concentrada,
//! coleta fees, remove imediatamente.

use alloy::primitives::{Address, U256, I256};
use tracing::info;

/// Pool V3/CL com dados necessários para JIT
#[derive(Clone, Debug)]
pub struct CLPool {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub tick: i32,
    pub liquidity: u128,
    pub sqrt_price_x96: U256,
    pub tvl_usd: f64,
}

/// Monitor de oportunidades JIT
#[derive(Debug)]
pub struct JITMonitor {
    /// Fee tier mínimo para considerar
    min_fee_bps: u32,
    /// TVL mínimo em USD
    min_tvl_usd: f64,
    /// Threshold de swap em ETH
    min_swap_eth: f64,
}

impl JITMonitor {
    pub fn new() -> Self {
        Self {
            min_fee_bps: 500,    // 0.05% min
            min_tvl_usd: 500_000.0, // $500k
            min_swap_eth: 1.0,   // 1 ETH
        }
    }

    /// Avalia se vale a pena fazer JIT para um swap
    /// Custo gas: ~300k para mint+burn+swap
    pub fn evaluate_opportunity(&self, pool: &CLPool, swap_amount_eth: f64, gas_price_gwei: f64) -> Option<JITOpportunity> {
        if pool.tvl_usd < self.min_tvl_usd {
            return None;
        }

        if swap_amount_eth < self.min_swap_eth {
            return None;
        }

        // Fee earned = amount * fee_tier
        // pool.fee vem em hundredths-of-a-bip (ex: 3000 = 0.3%, 500 = 0.05%),
        // a mesma unidade usada nativamente pelo Uniswap V3 / Aerodrome CL.
        // 1 unidade = 0.0001% = 1/1_000_000 -- por isso a divisão correta é
        // por 1_000_000, não por 10_000 (que estava a inflacionar a fee 100x:
        // ex. fee=3000 calculava 30% em vez de 0.3%, dando "2100 ETH de fee"
        // em swaps de ~7000 ETH onde o valor real seria ~21 ETH).
        let fee_earned_eth = swap_amount_eth * (pool.fee as f64) / 1_000_000.0;

        // Gas cost em ETH
        let gas_used = 300_000.0;
        let gas_cost_eth = gas_used * gas_price_gwei / 1_000_000_000.0;

        // Rentabilidade: fee > 2x gas
        if fee_earned_eth < gas_cost_eth * 2.0 {
            return None;
        }

        info!(
            "🎯 JIT Opportunity | Pool: {:?} | Fee: {:.6} ETH | Gas: {:.6} ETH",
            pool.address, fee_earned_eth, gas_cost_eth
        );

        Some(JITOpportunity {
            pool: pool.address,
            tick_lower: pool.tick - 10,
            tick_upper: pool.tick + 10,
            liquidity_to_add: self.calculate_liquidity_amount(pool, swap_amount_eth),
            expected_fee_eth: fee_earned_eth,
            gas_cost_eth,
        })
    }

    /// Calcula quantidade de liquidez a adicionar
    fn calculate_liquidity_amount(&self, pool: &CLPool, target_volume_eth: f64) -> u128 {
        // Simplificação: adicionar liquidez proporcional ao volume
        // L = volume / (2 * price)
        let current_liquidity = pool.liquidity as f64;
        let additional = (target_volume_eth * 1e18) / current_liquidity.max(1.0);

        additional.min(1e15) as u128 // Cap para não exagerar
    }
}

#[derive(Clone, Debug)]
pub struct JITOpportunity {
    pub pool: Address,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity_to_add: u128,
    pub expected_fee_eth: f64,
    pub gas_cost_eth: f64,
}
