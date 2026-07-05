with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''                        let optimal_input = optimal_cycle_input(
                            &best.hops,
                            MIN_FLASH_WEI_U256,
                            max_safe_input.max(MIN_FLASH_WEI_U256),
                            gas_cost_wei,
                        );
                        let opp_exec = Opportunity {'''

new = '''                        let optimal_input = optimal_cycle_input(
                            &best.hops,
                            MIN_FLASH_WEI_U256,
                            max_safe_input.max(MIN_FLASH_WEI_U256),
                            gas_cost_wei,
                        );

                        // CORREÇÃO DE CAUSA RAIZ (erro "IIA"): optimal_cycle_input usa a
                        // fórmula V2 (produto constante) para TODOS os hops, incluindo
                        // V3 -- estruturalmente errado para V3 (liquidez concentrada por
                        // tick, não uniforme). Isto sobrestimava sistematicamente quanto
                        // se podia trocar em hops V3, causando "IIA" no eth_call real
                        // (confirmado: 471/474 falhas numa sessão de 24min eram "IIA").
                        // Refinamento: se o ciclo tem algum hop V3, validar/reduzir o
                        // optimal_input via QuoterV2 oficial (simulação EXATA multi-tick,
                        // gratuita via eth_call) -- usa busca binária real em vez de
                        // qualquer margem de segurança adivinhada.
                        let has_v3_hop = best.hops.iter().any(|h| h.dex_type == crate::contracts::DexType::UniswapV3);
                        let optimal_input = if has_v3_hop {
                            if let Some(first_v3_hop) = best.hops.iter().find(|h| h.dex_type == crate::contracts::DexType::UniswapV3) {
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
                        };

                        let opp_exec = Opportunity {'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")
