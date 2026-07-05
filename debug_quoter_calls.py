with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

# Instrumentar find_max_viable_v3_input para confirmar chamadas
old1 = '''async fn find_max_viable_v3_input(
    provider: &impl AlloyProvider,
    token_in: Address,
    token_out: Address,
    fee: u32,
    max_candidate: U256,
) -> U256 {
    if max_candidate.is_zero() {
        return U256::ZERO;
    }'''

new1 = '''async fn find_max_viable_v3_input(
    provider: &impl AlloyProvider,
    token_in: Address,
    token_out: Address,
    fee: u32,
    max_candidate: U256,
) -> U256 {
    info!("[DIAG-QUOTER] find_max_viable_v3_input chamada: token_in={:?} token_out={:?} fee={} max_candidate={}", token_in, token_out, fee, max_candidate);
    if max_candidate.is_zero() {
        return U256::ZERO;
    }'''

count1 = content.count(old1)
print(f"Ocorrências 1: {count1}")

old2 = '''                        let has_v3_hop = best.hops.iter().any(|h| h.dex_type == crate::contracts::DexType::UniswapV3);'''

new2 = '''                        let has_v3_hop = best.hops.iter().any(|h| h.dex_type == crate::contracts::DexType::UniswapV3);
                        info!("[DIAG-QUOTER] has_v3_hop={} num_hops={}", has_v3_hop, best.hops.len());'''

count2 = content.count(old2)
print(f"Ocorrências 2: {count2}")

if count1 == 1 and count2 == 1:
    content = content.replace(old1, new1)
    content = content.replace(old2, new2)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso (ambos os pontos).")
else:
    print("ABORTADO -- alguma contagem != 1.")
