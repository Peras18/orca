//! 🎯 ExactOutputCalculator — Modo exact output para V3 swaps
//!
//! Em vez de especificar amountIn (que pode falhar por slippage),
//! especificamos amountOut e deixamos o pool calcular amountIn.
//! Isto garante que recebemos exactamente o que precisamos para o próximo hop
//! ou para repay do flash loan.

use alloy::primitives::U256;
use tracing::debug;

/// Modo de swap: ExactInput ou ExactOutput
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwapMode {
    /// Swap normal: especificamos quanto entra, calculamos saída
    ExactInput,
    /// Swap preciso: especificamos quanto queremos receber, pool calcula entrada
    ExactOutput,
}

/// Calcula amountIn necessário para obter amountOut exacto num pool V3
///
/// Fórmula (inversa do CPMM):
///   amountIn = (amountOut * reserveIn) / (reserveOut - amountOut) * (1 / (1 - fee))
pub fn compute_exact_input_for_output(
    amount_out: U256,
    reserve_in: U256,
    reserve_out: U256,
    fee_bps: u32,
) -> Option<U256> {
    if amount_out >= reserve_out {
        return None;
    }

    // CPMM inverso: dx = (dy * x) / (y - dy)
    let numerator = amount_out * reserve_in;
    let denominator = reserve_out - amount_out;

    if denominator.is_zero() {
        return None;
    }

    let amount_in_before_fee = numerator / denominator;

    // Ajustar pela fee: amountIn = amountIn_before_fee / (1 - fee)
    // fee_bps = 500 (0.05%) → multiplier = 10000 / (10000 - 5) = 1.0005
    let fee_adjustment = U256::from(10_000u32) * U256::from(10_000u32)
        / U256::from(10_000u32 - fee_bps);

    let amount_in = amount_in_before_fee * fee_adjustment / U256::from(10_000u32);

    debug!(
        "🎯 ExactOutput: para receber {} precisamos de {} (fee: {}bps)",
        amount_out, amount_in, fee_bps
    );

    Some(amount_in)
}

/// Para uma rota multi-hop, calcula o amountIn necessário de trás para frente
/// (backward propagation) para garantir que o último hop produza o suficiente
/// para repay do flash loan.
pub fn compute_backwards_multi_hop(
    desired_final_output: U256,
    reserves: &[(U256, U256)], // (reserve_in, reserve_out) para cada hop
    fees: &[u32],
) -> Option<U256> {
    let mut required_output = desired_final_output;

    // Propagar de trás para frente
    for i in (0..reserves.len()).rev() {
        let (r_in, r_out) = reserves[i];
        let fee = fees[i];

        required_output = compute_exact_input_for_output(required_output, r_in, r_out, fee)?;
    }

    debug!(
        "🎯 ExactOutput multi-hop: input necessário = {} para output = {}",
        required_output, desired_final_output
    );

    Some(required_output)
}

/// Verifica se uma rota exact-output é viável (não excede reserves)
pub fn is_exact_output_viable(
    amount_out: U256,
    reserve_out: U256,
    max_slippage_bps: u32,
) -> bool {
    let max_output = reserve_out * U256::from(max_slippage_bps) / U256::from(10_000u32);
    amount_out <= max_output
}
