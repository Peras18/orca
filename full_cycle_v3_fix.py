with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

# 1. Adicionar nova função: simula o ciclo completo hop a hop, usando
#    QuoterV2 real para hops V3 e a fórmula V2 para os restantes.
old1 = '''/// Encontra, via busca binária real contra o QuoterV2 (não aproximação),'''

new1 = '''/// 🎯 Simula o ciclo COMPLETO, hop a hop, devolvendo o output final real
/// (ou None se QUALQUER hop reverter). Usa QuoterV2 real para hops V3
/// (simulação exacta multi-tick) e a fórmula V2 para os restantes (AMMs
/// de produto constante já são bem representados por ela). O output de
/// cada hop alimenta o input do próximo, exactamente como a execução
/// real na chain -- corrige o bug de só validar o primeiro hop, que
/// deixava o 2º/3º hop sem nenhuma validação real (causa de "IIA"
/// persistir mesmo depois de refinar só o tamanho do flash loan inicial).
async fn simulate_full_cycle_v3_aware(
    provider: &impl AlloyProvider,
    hops: &[crate::graph::arb_graph::Edge],
    amount_in: U256,
) -> Option<U256> {
    let mut amount = amount_in;
    for hop in hops {
        if amount.is_zero() {
            return None;
        }
        amount = if hop.dex_type == crate::contracts::DexType::UniswapV3 {
            quote_v3_exact_input(provider, hop.token_in, hop.token_out, hop.fee, amount).await?
        } else {
            if hop.reserve_in.is_zero() || hop.reserve_out.is_zero() {
                return None;
            }
            let amount_in_with_fee = amount.saturating_mul(U256::from(997u64));
            let numerator = amount_in_with_fee.saturating_mul(hop.reserve_out);
            let denominator = hop
                .reserve_in
                .saturating_mul(U256::from(1000u64))
                .saturating_add(amount_in_with_fee);
            if denominator.is_zero() {
                return None;
            }
            numerator / denominator
        };
    }
    Some(amount)
}

/// Encontra, via busca binária sobre o CICLO COMPLETO (não só o 1º hop),
/// o maior amount_in que sobrevive a todos os hops em sequência real.
async fn find_max_viable_cycle_input(
    provider: &impl AlloyProvider,
    hops: &[crate::graph::arb_graph::Edge],
    max_candidate: U256,
) -> U256 {
    if max_candidate.is_zero() {
        return U256::ZERO;
    }
    if simulate_full_cycle_v3_aware(provider, hops, max_candidate).await.is_some() {
        return max_candidate;
    }
    let mut lo = U256::ZERO;
    let mut hi = max_candidate;
    for _ in 0..10 {
        if hi <= lo {
            break;
        }
        let mid = lo + (hi - lo) / U256::from(2u64);
        if mid.is_zero() {
            break;
        }
        if simulate_full_cycle_v3_aware(provider, hops, mid).await.is_some() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Encontra, via busca binária real contra o QuoterV2 (não aproximação),'''

count1 = content.count(old1)
print(f"Ocorrências 1: {count1}")
if count1 == 1:
    content = content.replace(old1, new1)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Nova função adicionada com sucesso.")
else:
    print("ABORTADO no ponto 1.")
