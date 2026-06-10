//! 🧮 Uniswap V3 Math — Virtual Reserves Approximation
//!
//! getAmountOut using virtual reserves (valid for 1-tick approximation):
//! fee_ppm: fee in parts per million (100 = 0.01%, 500 = 0.05%, 3000 = 0.3%)
//!
//! Formula: identical to V2 but with correct fee_ppm
//! amount_out = reserve_out * amount_in * (1_000_000 - fee_ppm)
//!              / (reserve_in * 1_000_000 + amount_in * (1_000_000 - fee_ppm))

use alloy::primitives::U256;

/// Calcula output Uniswap V3 usando reserves virtuais (aproximação single-tick)
/// fee_ppm: fee em partes por milhão (100 = 0.01%, 500 = 0.05%, 3000 = 0.3%)
pub fn get_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    fee_ppm: u32,
) -> Option<U256> {
    if reserve_in.is_zero() || reserve_out.is_zero() || amount_in.is_zero() {
        return None;
    }

    let fee_denom = U256::from(1_000_000u32);
    let fee_factor = fee_denom.checked_sub(U256::from(fee_ppm))?;

    let numerator = reserve_out
        .checked_mul(amount_in)?
        .checked_mul(fee_factor)?;

    let denominator = reserve_in
        .checked_mul(fee_denom)?
        .checked_add(amount_in.checked_mul(fee_factor)?)?;

    if denominator.is_zero() {
        return None;
    }

    numerator.checked_div(denominator)
}
