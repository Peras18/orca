//! 🧮 Uniswap V2 Math — Constant Product Formula
//!
//! getAmountOut: dy = (dx * 997 * y) / (x * 1000 + dx * 997)

use alloy::primitives::U256;

/// Calcula output para V2 (fee 0.3%)
pub fn get_amount_out_v2(amount_in: U256, reserve_in: U256, reserve_out: U256) -> U256 {
    if reserve_in.is_zero() || reserve_out.is_zero() {
        return U256::ZERO;
    }

    let amount_in_with_fee = amount_in.checked_mul(U256::from(997)).unwrap_or(U256::MAX);
    let numerator = amount_in_with_fee
        .checked_mul(reserve_out)
        .unwrap_or(U256::MAX);
    let denominator = reserve_in
        .checked_mul(U256::from(1000))
        .unwrap_or(U256::MAX)
        .checked_add(amount_in_with_fee)
        .unwrap_or(U256::MAX);

    if denominator.is_zero() {
        return U256::ZERO;
    }

    numerator / denominator
}

/// Calcula input necessário para output desejado
pub fn get_amount_in_v2(amount_out: U256, reserve_in: U256, reserve_out: U256) -> U256 {
    if reserve_in.is_zero() || reserve_out.is_zero() || amount_out >= reserve_out {
        return U256::ZERO;
    }

    let numerator = reserve_in
        .checked_mul(amount_out)
        .unwrap_or(U256::MAX)
        .checked_mul(U256::from(1000))
        .unwrap_or(U256::MAX);
    let denominator = reserve_out
        .checked_sub(amount_out)
        .unwrap_or(U256::MAX)
        .checked_mul(U256::from(997))
        .unwrap_or(U256::MAX);

    if denominator.is_zero() {
        return U256::ZERO;
    }

    (numerator / denominator).saturating_add(U256::from(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_amount_out() {
        let reserve_in = U256::from(1000);
        let reserve_out = U256::from(1000);
        let amount_in = U256::from(100);

        let out = get_amount_out_v2(amount_in, reserve_in, reserve_out);
        // Com fee 0.3%, espera ~90
        assert!(out > U256::ZERO && out < amount_in);
    }

    /// Teste com valores reais do log de produção (WETH/USDC Aerodrome vAMM):
    ///   reserve0 = 352 819 875 928 076 957 226  (WETH, 18 dec)
    ///   reserve1 = 823 755 331 012              (USDC, 6 dec)
    ///   amount_in = 10 000 000 000 000 000      (0.01 ETH)
    ///
    /// Resultado esperado: ≈23 300 000 USDC raw (≈23.3 USDC)
    /// Pior caso aceitável: qualquer valor > 0.
    #[test]
    fn test_real_weth_usdc_aerodrome() {
        let reserve_in = U256::from(352_819_875_928_076_957_226_u128);
        let reserve_out = U256::from(823_755_331_012_u128);
        let amount_in = U256::from(10_000_000_000_000_000_u128); // 0.01 ETH

        let out = get_amount_out_v2(amount_in, reserve_in, reserve_out);

        // Deve ser não-zero
        assert!(
            out > U256::ZERO,
            "get_amount_out_v2 retornou 0 para 0.01 ETH no pool WETH/USDC real"
        );

        // Deve estar na faixa razoável: entre 20 e 30 USDC raw
        // (20_000_000 ≤ out ≤ 30_000_000  com fee 0.30% e slippage desprezível)
        let min_expected = U256::from(20_000_000_u64); // 20 USDC raw
        let max_expected = U256::from(30_000_000_u64); // 30 USDC raw
        assert!(
            out >= min_expected && out <= max_expected,
            "get_amount_out_v2 = {} fora do intervalo esperado [20e6, 30e6] USDC raw",
            out
        );
    }

    /// Verifica que valores muito pequenos (abaixo do limiar de truncagem)
    /// retornam 0 — comportamento correcto, não um bug.
    #[test]
    fn test_small_amount_returns_zero_is_expected() {
        // Para reserve_in = 352 ETH e reserve_out = 823k USDC,
        // o limiar de truncagem é ≌4.3×10¹¹ wei.
        // Qualquer amount_in abaixo disto deve dar 0 por divisão inteira.
        let reserve_in = U256::from(352_819_875_928_076_957_226_u128);
        let reserve_out = U256::from(823_755_331_012_u128);
        let amount_in = U256::from(100_u64); // 100 wei = 10^-16 ETH

        let out = get_amount_out_v2(amount_in, reserve_in, reserve_out);
        // Resultado inteiro 0 é esperado aqui — não é um bug de overflow
        assert_eq!(
            out,
            U256::ZERO,
            "Expected 0 for 100-wei input into large pool (integer truncation, not a bug)"
        );
    }
}
