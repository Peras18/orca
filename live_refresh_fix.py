with open('src/main.rs', 'r') as f:
    content = f.read()

old = '''    {
        const SYNC_TOPIC: &str = "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1";
        let sync_topic: alloy::primitives::FixedBytes<32> = SYNC_TOPIC.parse().expect("hash Sync inválido");

        let wss_url = std::env::var("RPC_WSS_URLS")
            .unwrap_or_default()
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        if wss_url.is_empty() {
            warn!("[LIVE-REFRESH] RPC_WSS_URLS vazio -- actualização ao vivo desligada");
        } else {
            const CHUNK_SIZE: usize = 800;
            // Pools já cobertas por uma subscrição activa (lida/escrita só pela task de supervisão).
            let subscribed: Arc<dashmap::DashSet<alloy::primitives::Address>> =
                Arc::new(dashmap::DashSet::new());

            let pool_cache_super = pool_cache.clone();
            let wss_url_super = wss_url.clone();
            let subscribed_super = subscribed.clone();
            tokio::spawn(async move {
                let mut next_batch_idx: usize = 0;
                loop {
                    // Re-snapshot do cache: capta pools novas descobertas desde o último ciclo.
                    let all_addrs: Vec<alloy::primitives::Address> = pool_cache_super
                        .get_sample_pools(pool_cache_super.len())
                        .iter()
                        .map(|s| s.address)
                        .collect();

                    let new_addrs: Vec<alloy::primitives::Address> = all_addrs
                        .into_iter()
                        .filter(|a| !subscribed_super.contains(a))
                        .collect();

                    if !new_addrs.is_empty() {
                        let chunks: Vec<Vec<alloy::primitives::Address>> = new_addrs
                            .chunks(CHUNK_SIZE)
                            .map(|c| c.to_vec())
                            .collect();
                        info!(
                            "📡 [LIVE-REFRESH] {} pools novas detectadas -- a abrir {} lote(s) adicional(is)",
                            new_addrs.len(),
                            chunks.len()
                        );

                        for chunk in chunks {
                            for addr in &chunk {
                                subscribed_super.insert(*addr);
                            }
                            let idx = next_batch_idx;
                            next_batch_idx += 1;
                            let pool_cache_live = pool_cache_super.clone();
                            let wss_url_chunk = wss_url_super.clone();
                            tokio::spawn(async move {
                                loop {
                                    use alloy::providers::Provider as _;
                                    let conn = alloy::providers::ProviderBuilder::new()
                                        .on_ws(alloy::transports::ws::WsConnect::new(wss_url_chunk.clone()))
                                        .await;
                                    let provider = match conn {
                                        Ok(p) => p,
                                        Err(e) => {
                                            warn!("[LIVE-REFRESH] lote {} falhou a ligar: {} -- tentando de novo em 10s", idx, e);
                                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                                            continue;
                                        }
                                    };
                                    let filter = alloy::rpc::types::Filter::new()
                                        .address(chunk.clone())
                                        .event_signature(sync_topic);
                                    match provider.subscribe_logs(&filter).await {
                                        Ok(mut stream) => {
                                            info!("[LIVE-REFRESH] lote {} activo ({} pools)", idx, chunk.len());
                                            loop {
                                                match tokio::time::timeout(
                                                    tokio::time::Duration::from_secs(120),
                                                    stream.recv(),
                                                )
                                                .await
                                                {
                                                    Ok(Ok(log)) => {
                                                        let data = log.data().data.as_ref();
                                                        if data.len() >= 64 {
                                                            let r0 = alloy::primitives::U256::from_be_slice(&data[0..32]);
                                                            let r1 = alloy::primitives::U256::from_be_slice(&data[32..64]);
                                                            let block = log.block_number.unwrap_or(0);
                                                            pool_cache_live.update_sync_event(log.address(), r0, r1, block);
                                                        }
                                                    }
                                                    Ok(Err(_)) | Err(_) => break, // stream caiu ou ficou 120s sem nada -- reconectar
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("[LIVE-REFRESH] lote {} falhou subscrição: {}", idx, e);
                                        }
                                    }
                                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                }
                            });
                        }
                    }

                    // Intervalo de re-snapshot: discovery é contínuo, mas não é tão rápido
                    // que precise de verificação por-segundo -- 60s equilibra cobertura vs overhead.
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                }
            });
        }
    }'''

new = '''    {
        const SYNC_TOPIC: &str = "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1";
        let sync_topic: alloy::primitives::FixedBytes<32> = SYNC_TOPIC.parse().expect("hash Sync inválido");

        let wss_url = std::env::var("RPC_WSS_URLS")
            .unwrap_or_default()
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        if wss_url.is_empty() {
            warn!("[LIVE-REFRESH] RPC_WSS_URLS vazio -- actualização ao vivo desligada");
        } else {
            const CHUNK_SIZE: usize = 800;

            let pool_cache_super = pool_cache.clone();
            let wss_url_super = wss_url.clone();
            tokio::spawn(async move {
                // CORREÇÃO CRÍTICA v2: a versão anterior acumulava uma task
                // (e uma conexão WS própria) por cada lote de pools novas,
                // PARA SEMPRE -- depois de 10h chegou a 67 conexões WS
                // simultâneas, saturando o canal interno do pubsub e
                // colapsando silenciosamente a subscrição principal de
                // eventos (Status continuava "Connected", mas zero eventos
                // novos chegavam -- ~98% das execuções passaram a falhar por
                // "Block deadline exceeded" outra vez, mesmo com o timing já
                // corrigido). Agora: a cada re-snapshot, abortamos TODAS as
                // tasks antigas e relançamos um conjunto consolidado e
                // completo -- número de tasks vivas fica sempre limitado ao
                // necessário para a cobertura atual, nunca cresce sem fim.
                let mut active_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

                loop {
                    // Re-snapshot do cache: cobertura completa e actual, não incremental.
                    let all_addrs: Vec<alloy::primitives::Address> = pool_cache_super
                        .get_sample_pools(pool_cache_super.len())
                        .iter()
                        .map(|s| s.address)
                        .collect();

                    // Abortar todas as subscrições antigas antes de relançar --
                    // evita acumulação indefinida de conexões WS.
                    for handle in active_handles.drain(..) {
                        handle.abort();
                    }

                    if !all_addrs.is_empty() {
                        let chunks: Vec<Vec<alloy::primitives::Address>> = all_addrs
                            .chunks(CHUNK_SIZE)
                            .map(|c| c.to_vec())
                            .collect();
                        info!(
                            "📡 [LIVE-REFRESH] Reconsolidando {} pools em {} lote(s) (substituindo subscrições antigas)",
                            all_addrs.len(),
                            chunks.len()
                        );

                        for (idx, chunk) in chunks.into_iter().enumerate() {
                            let pool_cache_live = pool_cache_super.clone();
                            let wss_url_chunk = wss_url_super.clone();
                            let handle = tokio::spawn(async move {
                                loop {
                                    use alloy::providers::Provider as _;
                                    let conn = alloy::providers::ProviderBuilder::new()
                                        .on_ws(alloy::transports::ws::WsConnect::new(wss_url_chunk.clone()))
                                        .await;
                                    let provider = match conn {
                                        Ok(p) => p,
                                        Err(e) => {
                                            warn!("[LIVE-REFRESH] lote {} falhou a ligar: {} -- tentando de novo em 10s", idx, e);
                                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                                            continue;
                                        }
                                    };
                                    let filter = alloy::rpc::types::Filter::new()
                                        .address(chunk.clone())
                                        .event_signature(sync_topic);
                                    match provider.subscribe_logs(&filter).await {
                                        Ok(mut stream) => {
                                            info!("[LIVE-REFRESH] lote {} activo ({} pools)", idx, chunk.len());
                                            loop {
                                                match tokio::time::timeout(
                                                    tokio::time::Duration::from_secs(120),
                                                    stream.recv(),
                                                )
                                                .await
                                                {
                                                    Ok(Ok(log)) => {
                                                        let data = log.data().data.as_ref();
                                                        if data.len() >= 64 {
                                                            let r0 = alloy::primitives::U256::from_be_slice(&data[0..32]);
                                                            let r1 = alloy::primitives::U256::from_be_slice(&data[32..64]);
                                                            let block = log.block_number.unwrap_or(0);
                                                            pool_cache_live.update_sync_event(log.address(), r0, r1, block);
                                                        }
                                                    }
                                                    Ok(Err(_)) | Err(_) => break, // stream caiu ou ficou 120s sem nada -- reconectar
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("[LIVE-REFRESH] lote {} falhou subscrição: {}", idx, e);
                                        }
                                    }
                                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                }
                            });
                            active_handles.push(handle);
                        }
                    }

                    // Intervalo de re-snapshot e reconsolidação: 5 minutos equilibra
                    // cobertura de pools novas vs overhead de reconectar tudo.
                    tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                }
            });
        }
    }'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/main.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO -- contagem != 1.")
