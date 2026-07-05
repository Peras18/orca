with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''/// Pesquisa ternária: encontra o tamanho de input que maximiza o lucro líquido'''

new = '''/// 🎯 Simula um swap V3 single-tick usando a matemática REAL da curva
/// concentrada (não a aproximação de produto constante V2), usando os
/// dados já em cache (sqrt_price_x96, liquidity) -- sem nenhuma chamada
/// de rede extra, e sem depender de QuoterV2/Factory externos que podem
/// apontar para uma pool diferente da que realmente usamos (hop.pool).
///
/// Válido para swaps que não atravessam limites de tick -- para a maioria
/// das oportunidades de arbitragem MEV (tamanhos pequenos/médios face à
/// liquidez da pool), isto é uma aproximação muito mais fiel que a fórmula
/// V2, e gratuita (sem eth_call). Se o swap for grande o suficiente para
/// atravessar ticks, esta função pode sobrestimar ligeiramente o output --
/// a proteção final continua a ser o eth_call real do nosso próprio
/// contrato, que nunca gasta gás se a simulação falhar.
fn simulate_v3_single_tick(
    sqrt_price_x96: u128,
    liquidity: u128,
    decimals_in: u8,
    decimals_out: u8,
    zero_for_one: bool,
    amount_in: U256,
) -> Option<U256> {
    if liquidity == 0 || sqrt_price_x96 == 0 || amount_in.is_zero() {
        return None;
    }
    let l = U256::from(liquidity);
    let sqrt_p = U256::from(sqrt_price_x96);
    let q96 = U256::from(1u128) << 96;

    if zero_for_one {
        // Δ(1/√P) = amount_in / L  =>  √P_novo = (L * √P) / (L + amount_in * √P / 2^96)
        let numerator = l.checked_mul(sqrt_p)?;
        let amount_in_times_sqrt = amount_in.checked_mul(sqrt_p)?.checked_div(q96)?;
        let denominator = l.checked_add(amount_in_times_sqrt)?;
        if denominator.is_zero() {
            return None;
        }
        let sqrt_p_new = numerator.checked_div(denominator)?;
        if sqrt_p_new >= sqrt_p || sqrt_p_new.is_zero() {
            return None; // preço não pode subir ao vender token0, ou overflow
        }
        // amountOut = L * (√P - √P_novo) / 2^96
        let diff = sqrt_p.checked_sub(sqrt_p_new)?;
        let amount_out = l.checked_mul(diff)?.checked_div(q96)?;
        Some(amount_out)
    } else {
        // √P_novo = √P + (amount_in * 2^96) / L
        let amount_in_scaled = amount_in.checked_mul(q96)?.checked_div(l)?;
        let sqrt_p_new = sqrt_p.checked_add(amount_in_scaled)?;
        if sqrt_p_new <= sqrt_p {
            return None;
        }
        // amountOut = L * (1/√P - 1/√P_novo) = L * (√P_novo - √P) / (√P * √P_novo / 2^96)
        let diff = sqrt_p_new.checked_sub(sqrt_p)?;
        let numerator = l.checked_mul(diff)?.checked_mul(q96)?;
        let denominator = sqrt_p.checked_mul(sqrt_p_new)?;
        if denominator.is_zero() {
            return None;
        }
        Some(numerator.checked_div(denominator)?)
    }
    // NOTA: o resultado está na escala de decimais nativa do token de saída,
    // tal como os valores reais on-chain -- não precisa de ajuste extra
    // aqui, decimals_in/decimals_out ficam disponíveis para uso futuro se
    // necessário validação cruzada.
    .map(|out| { let _ = (decimals_in, decimals_out); out })
}

/// Pesquisa ternária: encontra o tamanho de input que maximiza o lucro líquido'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")
