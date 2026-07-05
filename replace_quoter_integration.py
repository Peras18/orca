with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''                        let has_v3_hop = best.hops.iter().any(|h| h.dex_type == crate::contracts::DexType::UniswapV3);
                        info!("[DIAG-QUOTER] has_v3_hop={} num_hops={}", has_v3_hop, best.hops.len());
                        let optimal_input = if has_v3_hop {
                            if let Some(first_v3_hop) = best.hops.iter().find(|h| h.dex_type == crate::contracts::DexType::UniswapV3) {
                                info!("[DIAG-QUOTER] find_max_viable_v3_input chamada: token_in={:?} token_out={:?} fee={} max_candidate={}", first_v3_hop.token_in, first_v3_hop.token_out, first_v3_hop.fee, optimal_input);
                                let refined = find_max_viable_v3_input(
                                    &*self.balance_provider,
                                    first_v3_hop.token_in,
                                    first_v3_hop.token_out,
                                    first_v3_hop.fee,
                                    optimal_input,
                                ).await;
                                if refined < optimal_input {
                                    debug!(
                                        "[QUOTER-V3] optimal_input refinado: {} -> {} wei (V2 sobrestimava liquidez V3 real)",
                                        optimal_input, refined
                                    );
                                }
                                refined.max(MIN_FLASH_WEI_U256)
                            } else {
                                optimal_input
                            }
                        } else {
                            optimal_input
                        };'''

new = '''                        let has_v3_hop = best.hops.iter().any(|h| h.dex_type == crate::contracts::DexType::UniswapV3);
                        let optimal_input = if has_v3_hop {
                            // CORREÇÃO v2: a versão anterior só validava o PRIMEIRO hop V3
                            // contra o QuoterV2 -- se o "IIA" vier do 2º ou 3º hop (cujo
                            // input depende do output do hop anterior, em cascata), essa
                            // validação parcial não detectava nada (confirmado: 64/64
                            // continuavam a falhar mesmo com a refinação do 1º hop activa).
                            // Agora valida o CICLO COMPLETO, hop a hop, com QuoterV2 real
                            // para cada hop V3 -- a mesma sequência exacta que a execução
                            // real na chain vai seguir.
                            let refined = find_max_viable_cycle_input(
                                &*self.balance_provider,
                                &best.hops,
                                optimal_input,
                            ).await;
                            if refined < optimal_input {
                                info!(
                                    "[QUOTER-V3] optimal_input refinado (ciclo completo): {} -> {} wei",
                                    optimal_input, refined
                                );
                            }
                            refined.max(MIN_FLASH_WEI_U256)
                        } else {
                            optimal_input
                        };'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")
