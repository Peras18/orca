//! 🧮 Input Optimizer — Optimal flash loan amounts
//!
//! Analítica exacta para 2-hop (sem iteração)
//! Grid + Newton-Raphson para 3-hop

use alloy::primitives::U256;

/// Calcula input óptimo para 2-hop usando fórmula analítica
/// Retorna (optimal_input, expected_profit)
pub fn find_optimal_input_2hop(
    r_in1: u128, r_out1: u128,   // pool 1
    r_in2: u128, r_out2: u128,   // pool 2
    f1: u128, f2: u128,          // fees: 997 para 0.3%, 9995 para 0.05%
    fee_denom: u128,             // 1000 ou 10000
) -> Option<(u128, u128)> {

    // Evitar overflow com números grandes
    // sqrt(r_in1 * r_out1 * r_in2 * r_out2)
    let r1: U256 = U256::from(r_in1).checked_mul(U256::from(r_out1))?;
    let r2: U256 = U256::from(r_in2).checked_mul(U256::from(r_out2))?;
    let product = r1.checked_mul(r2)?;
    let sqrt_product = isqrt(product)?;

    // sqrt(f1 * f2)
    let f_product = U256::from(f1).checked_mul(U256::from(f2))?;
    let sqrt_f = isqrt(f_product)?;

    // numerator = sqrt_product * sqrt_f / fee_denom - r_in1
    let num = sqrt_product
        .checked_mul(sqrt_f)?
        .checked_div(U256::from(fee_denom))?;

    if num <= U256::from(r_in1) {
        return None; // Sem lucro possível
    }

    let optimal = num - U256::from(r_in1);
    let max_safe = U256::from(r_in1) * U256::from(30) / U256::from(100); // 30% max impact
    let input = optimal.min(max_safe).try_into().unwrap_or(u128::MAX);

    // Calcular lucro real com o input escolhido
    let mid = get_amount_out_v2(input, r_in1, r_out1, f1, fee_denom)?;
    let out = get_amount_out_v2(mid, r_in2, r_out2, f2, fee_denom)?;

    if out <= input {
        return None;
    }

    Some((input, out - input))
}

/// Calcula input óptimo para 3-hop usando grid search + refinamento
pub fn find_optimal_input_3hop(
    pools: &[(u128, u128, u128, u128)], // [(r_in1,r_out1,r_in2,r_out2), ...]
    fees: &[(u128, u128)],              // [(f1,f2), (f3,f4), ...]
    fee_denom: u128,
) -> Option<(u128, u128)> {

    if pools.len() != 3 || fees.len() != 3 {
        return None;
    }

    // Grid search: testar 30 pontos logaritmicos
    let min_input = 1_000_000_000_000_000; // 0.001 ETH
    let max_input = 50_000_000_000_000_000; // 50 ETH
    let mut best_input = 0u128;
    let mut best_profit = 0u128;

    for i in 0..30 {
        // Input logaritmico: evita testar muitos valores pequenos
        let ratio = i as f64 / 29.0;
        let log_ratio = ratio * ratio; // Quadrático para dar mais peso aos maiores
        let input_f = min_input as f64 + (max_input - min_input) as f64 * log_ratio;
        let input = input_f as u128;

        let profit = simulate_3hop(input, pools, fees, fee_denom)?;
        if profit > best_profit {
            best_profit = profit;
            best_input = input;
        }
    }

    if best_profit == 0 {
        return None;
    }

    // Refinar com Newton-Raphson local (5 iterações)
    let refined = refine_3hop(best_input, pools, fees, fee_denom)?;

    Some(refined)
}

/// Simula 3-hop completo
fn simulate_3hop(
    input: u128,
    pools: &[(u128, u128, u128, u128)],
    fees: &[(u128, u128)],
    fee_denom: u128,
) -> Option<u128> {

    let mut amount = input;

    for (i, &(r_in, r_out, _, _)) in pools.iter().enumerate() {
        let (f_in, f_out) = fees[i];
        amount = get_amount_out_v2(amount, r_in, r_out, f_in, fee_denom)?;
    }

    if amount > input {
        Some(amount - input)
    } else {
        Some(0)
    }
}

/// Refina input óptimo com Newton-Raphson (aproximação)
fn refine_3hop(
    initial: u128,
    pools: &[(u128, u128, u128, u128)],
    fees: &[(u128, u128)],
    fee_denom: u128,
) -> Option<(u128, u128)> {

    let mut x = initial as f64;
    let step = 1_000_000_000_000.0; // 0.001 ETH step

    for _ in 0..5 {
        // Calcular profit em x
        let p1 = simulate_3hop(x as u128, pools, fees, fee_denom)? as f64;

        // Calcular profit em x + step
        let p2 = simulate_3hop((x + step) as u128, pools, fees, fee_denom)? as f64;

        // Derivada aproximada
        let dp_dx = (p2 - p1) / step;

        if dp_dx.abs() < 0.001 {
            break; // Máximo local encontrado
        }

        // Newton step: x = x + profit/dp_dx
        x += p1 / dp_dx;

        // Clamp aos limites
        x = x.max(1_000_000_000_000.0).min(100_000_000_000_000_000.0);
    }

    let input = x as u128;
    let profit = simulate_3hop(input, pools, fees, fee_denom)?;

    Some((input, profit))
}

/// Integer square root usando Newton-Raphson
/// Retorna floor(sqrt(x))
pub fn isqrt(x: U256) -> Option<U256> {
    if x.is_zero() {
        return Some(U256::ZERO);
    }

    let mut z = x;
    let mut y = U256::from(1);

    // Guess inicial: 2^(floor(log2(x))/2)
    let mut x_bits = 0u32;
    let mut temp = x;
    while temp > U256::ZERO {
        temp >>= 1;
        x_bits += 1;
    }
    let shift = (x_bits / 2) as usize;
    y <<= shift;

    // Newton-Raphson: y = (y + x/y) / 2
    for _ in 0..10 {
        let y_prev = y;
        y = (y + x / y) / U256::from(2);
        if y >= y_prev {
            break; // Convergiu
        }
    }

    Some(y)
}

/// Fórmula V2 get_amount_out (reutilizada)
fn get_amount_out_v2(
    amount_in: u128,
    reserve_in: u128,
    reserve_out: u128,
    fee: u128,
    fee_denom: u128,
) -> Option<u128> {

    if reserve_in == 0 || reserve_out == 0 {
        return Some(0);
    }

    let amount_in_with_fee = U256::from(amount_in).checked_mul(U256::from(fee))?;
    let numerator = amount_in_with_fee.checked_mul(U256::from(reserve_out))?;
    let denom_part = U256::from(reserve_in).checked_mul(U256::from(fee_denom))?;
    let denominator = denom_part.checked_add(amount_in_with_fee)?;

    if denominator.is_zero() {
        return Some(0);
    }

    Some((numerator / denominator).try_into().unwrap_or(u128::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_isqrt() {
        assert_eq!(isqrt(U256::from(4)), Some(U256::from(2)));
        assert_eq!(isqrt(U256::from(9)), Some(U256::from(3)));
        assert_eq!(isqrt(U256::from(16)), Some(U256::from(16)).map(|x| x.isqrt()));
    }

    #[test]
    fn test_2hop_optimizer() {
        // Pool 1: 1000 ETH -> 2M USDC (fee 0.3%)
        // Pool 2: 2M USDC -> 1000 ETH (fee 0.3%)
        let result = find_optimal_input_2hop(
            1000_000_000_000_000_000_000, // 1000 ETH
            2_000_000_000_000,           // 2M USDC
            2_000_000_000_000,           // 2M USDC
            1000_000_000_000_000_000_000, // 1000 ETH
            997, 997, 1000
        );

        assert!(result.is_some());
        let (input, profit) = result.unwrap();
        assert!(profit > 0);
    }
}
