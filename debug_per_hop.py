with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''async fn simulate_full_cycle_v3_aware(
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
}'''

new = '''async fn simulate_full_cycle_v3_aware(
    provider: &impl AlloyProvider,
    hops: &[crate::graph::arb_graph::Edge],
    amount_in: U256,
) -> Option<U256> {
    let mut amount = amount_in;
    for (hop_idx, hop) in hops.iter().enumerate() {
        if amount.is_zero() {
            info!("[DIAG-CYCLE] hop {} recebeu amount=0, abortando", hop_idx);
            return None;
        }
        amount = if hop.dex_type == crate::contracts::DexType::UniswapV3 {
            match quote_v3_exact_input(provider, hop.token_in, hop.token_out, hop.fee, amount).await {
                Some(out) => out,
                None => {
                    info!(
                        "[DIAG-CYCLE] hop {} (V3) FALHOU: token_in={:?} token_out={:?} fee={} amount_in={}",
                        hop_idx, hop.token_in, hop.token_out, hop.fee, amount
                    );
                    return None;
                }
            }
        } else {
            if hop.reserve_in.is_zero() || hop.reserve_out.is_zero() {
                info!("[DIAG-CYCLE] hop {} (não-V3) reserves zero", hop_idx);
                return None;
            }
            let amount_in_with_fee = amount.saturating_mul(U256::from(997u64));
            let numerator = amount_in_with_fee.saturating_mul(hop.reserve_out);
            let denominator = hop
                .reserve_in
                .saturating_mul(U256::from(1000u64))
                .saturating_add(amount_in_with_fee);
            if denominator.is_zero() {
                info!("[DIAG-CYCLE] hop {} (não-V3) denominator zero", hop_idx);
                return None;
            }
            numerator / denominator
        };
    }
    Some(amount)
}'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")
