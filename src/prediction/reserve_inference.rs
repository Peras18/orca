use alloy::primitives::U256;

const ONE_E18: u128 = 1_000_000_000_000_000_000u128;

/// Converte sqrtPriceX96 (Q64.96) para preço normalizado (18 decimais).
/// Fórmula: price = (sqrtPrice / 2^96)^2
pub fn sqrt_price_to_price(sqrt_price_x96: U256) -> u128 {
    // Q96 = 2^96
    let q96: U256 = U256::from(1u128) << 96;
    if q96.is_zero() || sqrt_price_x96.is_zero() {
        return 0;
    }

    // price_x192 = sqrtPrice^2 (Q128.192)
    let price_x192 = sqrt_price_x96.saturating_mul(sqrt_price_x96);

    // price = (sqrt^2 / 2^192) * 1e18
    let denom = q96.saturating_mul(q96);
    let price_norm = price_x192
        .saturating_mul(U256::from(ONE_E18))
        / denom.max(U256::from(1u8));

    price_norm.try_into().unwrap_or(u128::MAX)
}

fn normalize_to_18(amount: u128, decimals: u8) -> u128 {
    if decimals == 18 {
        amount
    } else if decimals < 18 {
        amount.saturating_mul(10u128.saturating_pow((18u32 - decimals as u32) as u32))
    } else {
        amount / 10u128.saturating_pow((decimals as u32 - 18u32) as u32).max(1)
    }
}

/// Detecta divergência entre preço V3 e pool V2 equivalente.
/// Retorna divergência em basis points (1 bp = 0.01%).
pub fn detect_cross_pool_divergence(
    v3_sqrt_price: U256,
    v2_reserve0: u128,
    v2_reserve1: u128,
    decimals0: u8,
    decimals1: u8,
) -> u32 {
    if v2_reserve0 == 0 || v2_reserve1 == 0 {
        return 0;
    }

    let v3_price = sqrt_price_to_price(v3_sqrt_price);
    if v3_price == 0 {
        return 0;
    }

    // v2_price = (reserve1/reserve0) normalizado para 18 decimais
    let r0 = normalize_to_18(v2_reserve0, decimals0);
    let r1 = normalize_to_18(v2_reserve1, decimals1);
    if r0 == 0 || r1 == 0 {
        return 0;
    }

    let v2_price = (U256::from(r1).saturating_mul(U256::from(ONE_E18))
        / U256::from(r0).max(U256::from(1u8)))
    .try_into()
    .unwrap_or(u128::MAX);

    if v2_price == 0 {
        return 0;
    }

    let diff = if v3_price > v2_price {
        v3_price - v2_price
    } else {
        v2_price - v3_price
    };

    (diff.saturating_mul(10_000) / v2_price.max(1)) as u32
}

