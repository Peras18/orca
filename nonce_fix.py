import sys

path = sys.argv[1]
with open(path, 'r') as f:
    content = f.read()

old = '''        // CORREÇÃO: preferir um RPC privado (Tenderly/QuickNode) sobre o público
        // mainnet.base.org -- esse rate-limita (HTTP 429) sob qualquer carga
        // real, abortando tentativas válidas por excesso de chamadas, não por
        // revert genuíno. Escolhe o primeiro URL da lista que NÃO seja o público.
        let rpc_list = std::env::var("RPC_HTTP_URLS").unwrap_or_default();
        let chosen_rpc = rpc_list
            .split(',')
            .map(|s| s.trim())
            .find(|u| !u.is_empty() && !u.contains("mainnet.base.org"))
            .or_else(|| rpc_list.split(',').map(|s| s.trim()).find(|u| !u.is_empty()))
            .unwrap_or("https://mainnet.base.org");
        let http_url: reqwest::Url = chosen_rpc
            .to_string()
            .parse()
            .unwrap_or_else(|_| "https://mainnet.base.org".parse().unwrap());
        let provider = alloy::providers::ProviderBuilder::new()
            .wallet(wallet)
            .on_http(http_url);
        let from_addr = signer.address();
        let nonce = match provider.get_transaction_count(from_addr).await {
            Ok(n) => n,
            Err(e) => { warn!("[ORCA] ❌ Falha ao obter nonce: {}", e); return None; }
        };'''

new = '''        // CORREÇÃO: preferir RPCs privados (Tenderly/QuickNode) sobre o público
        // mainnet.base.org -- esse rate-limita (HTTP 429) sob qualquer carga
        // real, abortando tentativas válidas por excesso de chamadas, não por
        // revert genuíno. Privados primeiro, público como último recurso.
        let rpc_list = std::env::var("RPC_HTTP_URLS").unwrap_or_default();
        let mut rpc_candidates: Vec<String> = rpc_list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|u| !u.is_empty() && !u.contains("mainnet.base.org"))
            .collect();
        rpc_candidates.push("https://mainnet.base.org".to_string());

        let from_addr = signer.address();

        // CORREÇÃO: um único RPC privado (Tenderly) estava a sofrer timeouts
        // (HTTP 408/504) sob carga, abortando ~35% das tentativas de execução
        // antes mesmo de chegar ao eth_call -- nenhuma TX real chegava a ser
        // enviada nesses casos. Agora tenta cada RPC da lista em sequência
        // até um responder, em vez de desistir ao primeiro timeout.
        let mut provider_opt = None;
        let mut nonce_opt = None;
        for rpc_url in &rpc_candidates {
            let http_url: reqwest::Url = match rpc_url.parse() {
                Ok(u) => u,
                Err(_) => continue,
            };
            let candidate_provider = alloy::providers::ProviderBuilder::new()
                .wallet(wallet.clone())
                .on_http(http_url);
            match tokio::time::timeout(
                std::time::Duration::from_millis(800),
                candidate_provider.get_transaction_count(from_addr),
            ).await {
                Ok(Ok(n)) => {
                    nonce_opt = Some(n);
                    provider_opt = Some(candidate_provider);
                    break;
                }
                Ok(Err(e)) => {
                    warn!("[ORCA] ⚠️ RPC {} falhou ao obter nonce: {} -- tentando próximo", rpc_url, e);
                }
                Err(_) => {
                    warn!("[ORCA] ⚠️ RPC {} timeout (800ms) ao obter nonce -- tentando próximo", rpc_url);
                }
            }
        }

        let provider = match provider_opt {
            Some(p) => p,
            None => { warn!("[ORCA] ❌ Todos os RPCs falharam ao obter nonce"); return None; }
        };
        let nonce = match nonce_opt {
            Some(n) => n,
            None => { warn!("[ORCA] ❌ Todos os RPCs falharam ao obter nonce"); return None; }
        };'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open(path, 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO -- contagem != 1.")
