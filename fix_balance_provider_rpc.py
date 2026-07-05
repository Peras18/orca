with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''        let balance_provider = alloy::providers::builder()
            .on_http(
                config
                    .base_rpc_url
                    .parse()
                    .expect("base_rpc_url inválida para provider HTTP"),
            )
            .boxed();'''

new = '''        // CORREÇÃO: balance_provider usava config.base_rpc_url, cujo default
        // é mainnet.base.org -- RPC público que rate-limita (HTTP 429) sob
        // carga real. Isto era usado também pelo QuoterV2 (find_max_viable_
        // cycle_input faz várias chamadas eth_call por tentativa), e os 429
        // estavam a ser interpretados como "IIA real" (liquidez insuficiente)
        // quando eram apenas rate-limit do RPC -- causa provável de
        // "optimal_input refinado: X -> 0 wei" em 100% dos casos observados.
        // Agora usa o primeiro RPC privado de RPC_HTTP_URLS, com o mesmo
        // padrão de fallback já usado em submit_to_protector.
        let chosen_http_rpc = std::env::var("RPC_HTTP_URLS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .find(|u| !u.is_empty() && !u.contains("mainnet.base.org"))
            .unwrap_or_else(|| config.base_rpc_url.clone());
        let balance_provider = alloy::providers::builder()
            .on_http(
                chosen_http_rpc
                    .parse()
                    .expect("RPC_HTTP_URLS/base_rpc_url inválida para provider HTTP"),
            )
            .boxed();'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")
