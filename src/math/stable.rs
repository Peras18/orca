//! 🧪 Aerodrome Stable AMM Math (x³y + xy³ = k) — Versão Anti-Overflow
//!
//! 🚨 CORREÇÃO CRÍTICA: U256 com fórmula fatorizada para evitar overflow

use alloy::primitives::U256;

/// Precisão de 18 decimais (1e18) — u128 para cálculos internos
pub const PRECISION: u128 = 1_000_000_000_000_000_000;

/// 🚨 CORREÇÃO: get_k_stable anti-overflow com U256 fatorizado
/// Fórmula: k = x*y * (x² + y²) / PRECISION³
pub fn get_k_stable(x: U256, y: U256) -> U256 {
    let x_norm = normalize(x);
    let y_norm = normalize(y);
    let precision = U256::from(10).pow(U256::from(18));

    // Fórmula Fatorizada: evita x³y direto
    let x2 = x_norm.saturating_mul(x_norm) / precision;
    let y2 = y_norm.saturating_mul(y_norm) / precision;
    let xy = x_norm.saturating_mul(y_norm) / precision;

    xy.saturating_mul(x2.saturating_add(y2)) / precision
}

/// Wrapper u128 para compatibilidade
pub fn get_k_stable_u128(x: u128, y: u128) -> u128 {
    let x_u256 = U256::from(x);
    let y_u256 = U256::from(y);
    let result = get_k_stable(x_u256, y_u256);
    // Limitar a u128::MAX se necessário
    result.try_into().unwrap_or(u128::MAX)
}

/// Normaliza um valor em wei para unidades base (dividindo por 10¹⁸)
#[inline]
fn normalize(value: U256) -> U256 {
    let precision = U256::from(10).pow(U256::from(18));
    value / precision
}

/// Encontra y tal que get_k_stable(x, y) = k via Newton-Raphson.
///
/// 🚨 CORREÇÃO DE OVERFLOW: versão anterior usava u128 para x²/y².
/// Com reserves típicas (ex. WETH: 3.5×10²⁰ ou USDC normalizado: 8.2×10²³),
/// x.saturating_mul(x) excede u128::MAX (≈3.4×10³⁸) — resultado saturado
/// invalida toda a iteração Newton-Raphson.
///
/// SOLUÇÃO: todos os intermediários de quadratura e multiplicação em U256
/// (max ≈1.15×10⁷⁷); apenas a saída final é convertida de volta para u128.
pub fn get_y_stable(x: u128, k: u128, y_init: u128) -> u128 {
    let prec = U256::from(PRECISION); // 10¹⁸
    let x256 = U256::from(x);
    let k256 = U256::from(k);

    // x² / PRECISION  — em U256, sem overflow
    // Para x = 3.5×10²⁰:  x² = 1.2×10⁴¹, x²/10¹⁸ = 1.2×10²³ ✓
    let x2 = x256 * x256 / prec;

    let mut y256 = U256::from(y_init);

    for _ in 0..255 {
        let y_prev = y256;
        let y2 = y256 * y256 / prec; // y² / PRECISION, sem overflow

        // k0 = xy(x² + y²) / PRECISION²
        let xy = x256 * y256 / prec;
        let k0 = xy * (x2 + y2) / prec;

        // derivada de k em ordem a y:  x * (3y² + x²) / PRECISION
        let deriv = x256 * (U256::from(3u64) * y2 + x2) / prec;

        if deriv.is_zero() {
            break;
        }

        // Passo Newton-Raphson
        if k0 > k256 {
            let delta = (k0 - k256) / deriv;
            y256 = y256.saturating_sub(delta);
        } else {
            let delta = (k256 - k0) / deriv;
            y256 = y256 + delta;
        }

        // Convergência: diff <= 1 unit
        let diff = if y256 > y_prev {
            y256 - y_prev
        } else {
            y_prev - y256
        };
        if diff <= U256::from(1u64) {
            break;
        }
    }

    // Converter de volta para u128; saturar se > u128::MAX
    y256.try_into().unwrap_or(u128::MAX)
}

/// 🚨 CORREÇÃO: get_amount_out_stable com normalização de decimais
/// Fee: 0.01% (Aerodrome sAMM) = 9999/10000
pub fn get_amount_out_stable(
    amount_in: u128,
    reserve_in: u128,
    reserve_out: u128,
    decimals_in: u8,
    decimals_out: u8,
) -> Option<u128> {
    if amount_in == 0 || reserve_in == 0 || reserve_out == 0 {
        return None;
    }

    // Normaliza para 18 decimais
    let scale = |a: u128, d: u8| -> u128 {
        match d.cmp(&18) {
            std::cmp::Ordering::Less => a.saturating_mul(10u128.pow((18 - d) as u32)),
            std::cmp::Ordering::Greater => a / 10u128.pow((d - 18) as u32),
            std::cmp::Ordering::Equal => a,
        }
    };

    let x = scale(reserve_in, decimals_in);
    let y = scale(reserve_out, decimals_out);

    // Fee 0.01% Aerodrome sAMM
    let dx = scale(amount_in, decimals_in).saturating_mul(9999) / 10_000;

    let k = get_k_stable_u128(x, y);
    let y_new = get_y_stable(x.saturating_add(dx), k, y);

    if y <= y_new {
        return None;
    }

    let dy_norm = y - y_new;

    // De-normalizar para decimals_out
    let dy = match decimals_out.cmp(&18) {
        std::cmp::Ordering::Less => dy_norm / 10u128.pow((18 - decimals_out) as u32),
        std::cmp::Ordering::Greater => {
            dy_norm.saturating_mul(10u128.pow((decimals_out - 18) as u32))
        }
        std::cmp::Ordering::Equal => dy_norm,
    };

    if dy == 0 {
        None
    } else {
        Some(dy)
    }
}

/// Wrapper U256 para compatibilidade com código existente

pub fn get_amount_out_stable_u256(amount_in: U256, reserve_in: U256, reserve_out: U256) -> U256 {
    // Converter para u128 (assume que valores cabem em u128 após normalização)
    let ai: u128 = amount_in.try_into().unwrap_or(u128::MAX);
    let ri: u128 = reserve_in.try_into().unwrap_or(u128::MAX);
    let ro: u128 = reserve_out.try_into().unwrap_or(u128::MAX);

    match get_amount_out_stable(ai, ri, ro, 18, 18) {
        Some(out) => U256::from(out),
        None => U256::ZERO,
    }
}
