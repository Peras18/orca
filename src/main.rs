#![allow(dead_code, unused_variables)]

use std::sync::Arc;
// use std::path::Path; // Not used
use tracing::{error, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use orca_mev::apex_shadow_protocol::ApexShadowProtocol;
use orca_mev::artemis::{
    ApexConfig, ApexPredatorEngine, ArtemisEngine, CollectorConfigV2, EventFilter, LogCollectorV2,
    StrategyContext,
};
use orca_mev::config::AppConfig;
use orca_mev::contracts::{AerodromeFactory, DexType, UniswapV3Factory};
use orca_mev::discovery::{DiscoveryConfig, PoolDiscoveryEngine};
use orca_mev::empire::EmpireFoundationEngine;
use orca_mev::provider::Provider;
use orca_mev::singularity::SingularityMEV;
use orca_mev::strategist::ProfitConfig;
use orca_mev::strategist::{HighPerformanceStrategist, StrategistConfig};
use orca_mev::telemetry::{spawn_telemetry_printer, TelemetryCollector};
use orca_mev::EngineConfig;

/// Verifica Chain ID e moradas das fábricas na Base Mainnet
fn verify_chain_configuration() {
    info!("═══════════════════════════════════════════════════════════");
    info!("🔍 VERIFICAÇÃO DE CONFIGURAÇÃO BASE MAINNET");
    info!("═══════════════════════════════════════════════════════════");

    // Chain ID fixo em 8453 (Base Mainnet)
    const EXPECTED_CHAIN_ID: u64 = 8453;
    info!(
        "✅ Chain ID configurado: {} (Base Mainnet)",
        EXPECTED_CHAIN_ID
    );

    // Verificar morada da Factory Uniswap V3 na Base
    let uni_v3_factory = UniswapV3Factory::ADDRESS;
    info!("✅ Uniswap V3 Factory: {:?}", uni_v3_factory);
    // Morada oficial: 0x33128a8fC17869897dcE68Ed026d694621f6FDfD

    // Verificar morada da Factory Aerodrome na Base
    let aerodrome_factory = AerodromeFactory::BASE_MAINNET;
    info!("✅ Aerodrome Factory: {:?}", aerodrome_factory);
    // Morada oficial: 0x42024DAB8ED9bcE086865ACd50831A567Bb4258B

    info!("═══════════════════════════════════════════════════════════");
}

fn setup_logging() -> (WorkerGuard, WorkerGuard) {
    // Criar diretório de logs se não existir
    std::fs::create_dir_all("logs").ok();

    // File logger — rotação diária, só WARN
    let file_appender = tracing_appender::rolling::daily("logs", "shadow_hunter");
    let (non_blocking_file, file_guard) = tracing_appender::non_blocking(file_appender);

    // Terminal logger — rotação diária, só WARN
    let terminal_appender = tracing_appender::rolling::daily("logs", "terminal");
    let (non_blocking_terminal, terminal_guard) = tracing_appender::non_blocking(terminal_appender);
    // Console logger para stdout
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true);

    // EnvFilter para controlar nível de log
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("orca_mev=info,info"));

    // File layer para arquivo persistente
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_file)
        .with_ansi(false) // Sem cores no arquivo
        .with_target(true)
        .with_thread_ids(true);

    // Terminal layer - captura TUDO num ficheiro específico
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_terminal)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);

    // Combinar todos os layers
    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .with(terminal_layer)
        .init();

    info!("📁 File logger inicializado: logs/shadow_hunter_results.log");
    info!("📁 Terminal logger ativo: logs/last_terminal.log");

    // Guards devem ser retornados para manter os workers vivos
    (file_guard, terminal_guard)
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Carregar .env (fail-fast se obrigatórias em falta)
    let app_config = AppConfig::load();

    // 2. Configurar logging persistente
    let (_file_guard, _terminal_guard) = setup_logging();

    // Verificar DEBUG_MODE
    let debug_mode = std::env::var("DEBUG_MODE").unwrap_or_default() == "true";
    if debug_mode {
        info!("========================================");
        info!("StrategyEngine ativado");
        info!("========================================");
    }

    info!("ApexBaseMEV Bot v2.0 - HFT Infrastructure");
    info!("Starting high-performance MEV engine for Base network...");

    // Verificar configuração de chain
    verify_chain_configuration();

    // Converter AppConfig para EngineConfig
    let config = EngineConfig {
        region: Box::leak(app_config.region.clone().into_boxed_str()),
        max_path_length: app_config.max_path_length,
        min_profit_basis_points: app_config.min_profit_basis_points,
        dry_run: app_config.dry_run,
        enable_backrun: app_config.enable_backrun,
    };

    if config.dry_run {
        info!("🔍 MODO SHADOW HUNTER ATIVO: Simulação sem execução real");
    }
    if config.enable_backrun {
        info!("🏃 State Overlay para backrunning ativado");
    }

    // Provider com WebSocket Alchemy
    let provider = Arc::new(Provider::new(&config, &app_config).await?);

    // DISCOVERY V3 HÍBRIDO - bootstrap histórico + realtime
    info!("[SYSTEM] Iniciando discovery em modo agressivo (lookback otimizado).");
    let lookback_blocks = std::env::var("SCAN_BLOCKS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1000); // Aumentado para 1000 para garantir quórum mínimo sem loop
    info!(
        "[DISCOVERY] Janela de scan configurada: {} blocos",
        lookback_blocks
    );

    let discovery_config = DiscoveryConfig {
        min_tvl_usd: 100_000.0, // Baixado para $100k para capturar "brechas"
        min_volume_24h_usd: 5_000.0,
        max_pools: 5000,
        scan_interval_secs: 300,
        lookback_blocks,
    };

    let discovery_engine = Arc::new(PoolDiscoveryEngine::new(
        provider.inner().clone(),
        discovery_config,
    ));

    // Sincronismo total: não inicia Collector/Engine sem bootstrap pronto.
    let bootstrap_target = std::env::var("DISCOVERY_READY_TARGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(30); // Baixado de 60 para 30 para atingir quórum imediato
                        // Mínimo absoluto para operação: 4 pools (seed pools garantidas).
                        // Com seeds + elite hardcoded o bot arranca com 37+ pools.
                        // Só falha se até os seeds não carregarem (impossível em condições normais).
    const MIN_POOLS_FOR_OPERATION: usize = 4;

    info!(
        "[DISCOVERY] Aguardando bootstrap síncrono. Alvo: {} pools | Mínimo operacional: {} pools",
        bootstrap_target, MIN_POOLS_FOR_OPERATION
    );
    let pool_count = discovery_engine
        .initialize_sync(bootstrap_target, 180)
        .await?;

    // 🚀 GRACEFUL DEGRADATION: Não abortar se tivermos pools suficientes para operar
    if pool_count < MIN_POOLS_FOR_OPERATION {
        return Err(eyre::eyre!(
            "[DISCOVERY] 🚨 BOOTSTRAP CRÍTICO: {} < {} pools mínimas. Sem liquidez suficiente para operar.",
            pool_count, MIN_POOLS_FOR_OPERATION
        ));
    } else if pool_count < bootstrap_target {
        // ⚠️ Modo degradado: prosseguir com aviso, não erro fatal
        warn!(
            "[DISCOVERY] ⚠️  MODO DEGRADADO: {} pools (alvo: {}). Prosseguindo com liquidez limitada.",
            pool_count, bootstrap_target
        );
        warn!(
            "[DISCOVERY] ⚠️  O failed_state_speculator irá operar com {} pools elite/hardcoded.",
            pool_count
        );
    } else {
        info!(
            "[DISCOVERY] ✅ Prontidão confirmada: {} pools carregadas (>= {}).",
            pool_count, bootstrap_target
        );
    }

    // Salvar cache atualizado
    // Carregar cache de pools descobertos anteriormente
    match discovery_engine.load_from_cache().await {
        Ok(n) => info!("[CACHE] {} pools carregados do cache persistente", n),
        Err(e) => warn!("[CACHE] Erro ao carregar cache: {}", e),
    }
    if let Err(e) = discovery_engine.save_to_cache().await {
        warn!("[CACHE] Erro ao salvar cache: {}", e);
    } else {
        info!("[CACHE] Cache atualizado com {} pools", pool_count);
    }

    // 🚨 CORREÇÃO: Criar pool_cache GLOBAL antes do bootstrap (usado também no event processor)
    use orca_mev::cache::PoolCache;
    const RESERVES_CACHE_PATH: &str = "pool_reserves_cache.json";
    let pool_cache = Arc::new(
        match tokio::fs::read_to_string(RESERVES_CACHE_PATH).await {
            Ok(data) => match PoolCache::from_json(&data) {
                Ok(loaded) => {
                    info!("📦 [RESERVES-CACHE] {} pools com reserves carregados do disco — sem recomeçar do zero", loaded.len());
                    loaded
                }
                Err(e) => {
                    warn!("⚠️ [RESERVES-CACHE] Falha ao parsear cache em disco ({}), a começar vazio", e);
                    PoolCache::new()
                }
            },
            Err(_) => {
                info!("📦 [RESERVES-CACHE] Sem cache em disco ainda — primeiro arranque");
                PoolCache::new()
            }
        }
    );

    // Guardar reserves periodicamente — para nunca mais perder progresso de bootstrap num restart
    {
        let pool_cache_save = pool_cache.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                tick.tick().await;
                match pool_cache_save.to_json() {
                    Ok(json) => {
                        if let Err(e) = tokio::fs::write(RESERVES_CACHE_PATH, json).await {
                            warn!("⚠️ [RESERVES-CACHE] Falha ao guardar: {}", e);
                        } else {
                            info!("💾 [RESERVES-CACHE] {} pools guardados em disco", pool_cache_save.len());
                        }
                    }
                    Err(e) => warn!("⚠️ [RESERVES-CACHE] Falha ao serializar: {}", e),
                }
            }
        });
    }

    // Bootstrap: Multicall3 primeiro (rapido -- 1 RPC call para N pools),
    // com fallback automatico para chamadas individuais se falhar ou nao
    // inicializar nenhuma pool.
    info!("🚀 [BOOTSTRAP] A tentar Multicall3 (rápido), com fallback para individual...");
    let pool_addresses = discovery_engine.get_all_pool_addresses().await;

    // CORREÇÃO: pré-popular o cache com token0/token1/fee/dex_type reais ANTES do
    // Multicall3 -- sem isto, process_batch() não encontra a pool no cache (Some(state)
    // falha) e a inicialização fica sempre em 0, mesmo com a chamada RPC a funcionar.
    {
        use orca_mev::cache::pool_cache::PoolState;
        let pool_data_list = discovery_engine.get_all_pool_data().await;
        let mut prepopulated = 0u64;
        for pd in pool_data_list {
            if !pool_cache.contains(&pd.address) {
                // Converter DexType de discovery::DexType para contracts::DexType
                // (são dois enums distintos, mesmo nome, módulos diferentes)
                let dex_type = match pd.dex_type {
                    orca_mev::discovery::DexType::UniswapV3 => orca_mev::contracts::DexType::UniswapV3,
                    orca_mev::discovery::DexType::UniswapV2 => orca_mev::contracts::DexType::UniswapV2,
                    orca_mev::discovery::DexType::Aerodrome => orca_mev::contracts::DexType::Aerodrome,
                };
                pool_cache.insert(PoolState::new(pd.address, pd.token0, pd.token1, pd.fee, dex_type));
                prepopulated += 1;
            }
        }
        info!("📦 [BOOTSTRAP] {} pools pré-populadas no cache (token0/token1 reais)", prepopulated);
    }

    use orca_mev::cache::{bootstrap_simple, MulticallBootstrap, BootstrapConfig};
    let bootstrap_provider = {
        let provider_handle = provider.inner();
        let provider_clone = {
            let guard = provider_handle.read().await;
            guard.clone()
        };
        Arc::new(provider_clone)
    };

    let multicall_provider_rwlock = Arc::new(tokio::sync::RwLock::new((*bootstrap_provider).clone()));
    let multicall = MulticallBootstrap::new(
        multicall_provider_rwlock,
        pool_cache.clone(),
        BootstrapConfig::default(),
    );

    let bootstrap_final_result: eyre::Result<usize> = match multicall
        .bootstrap_reserves(&pool_addresses)
        .await
    {
        Ok(n) if n > 0 => {
            info!("🚀 [BOOTSTRAP] Multicall3 inicializou {} pools rapidamente", n);
            Ok(n)
        }
        Ok(_) => {
            warn!("⚠️ [BOOTSTRAP] Multicall3 inicializou 0 pools, a usar fallback individual");
            bootstrap_simple(bootstrap_provider.clone(), pool_cache.clone(), &pool_addresses).await
        }
        Err(e) => {
            warn!("⚠️ [BOOTSTRAP] Multicall3 falhou ({}), a usar fallback individual", e);
            bootstrap_simple(bootstrap_provider.clone(), pool_cache.clone(), &pool_addresses).await
        }
    };

    match bootstrap_final_result {
        Ok(initialized) => {
            info!(
                "🎉 [BOOTSTRAP] {} pools inicializadas com sucesso",
                initialized
            );

            // 🔬 DIAGNÓSTICO: Verificar reserves no cache
            let pools_with_reserves = pool_cache.count_pools_with_reserves();
            info!(
                "[BOOTSTRAP] Pools com reserves válidas: {}",
                pools_with_reserves
            );

            // Mostrar as primeiras 3 pools COM reserves reais (skip placeholders zeros)
            let sample_with_reserves: Vec<_> = pool_cache
                .get_sample_pools(pool_cache.len())
                .into_iter()
                .filter(|s| !s.reserve0.is_zero() && !s.reserve1.is_zero())
                .take(3)
                .collect();
            for (i, state) in sample_with_reserves.iter().enumerate() {
                info!(
                    "[BOOTSTRAP] Pool {}: addr={:?} | r0={} | r1={} | t0={:?} | t1={:?} | dex={:?}",
                    i,
                    state.address,
                    state.reserve0,
                    state.reserve1,
                    state.token0,
                    state.token1,
                    state.dex_type
                );
            }

            // Alerta se reserves são muito pequenas (possível erro de decode)
            use alloy::primitives::U256;
            let tiny_reserves = pool_cache
                .get_sample_pools(pool_cache.len())
                .iter()
                .filter(|state| {
                    let r0_tiny =
                        state.reserve0 > U256::ZERO && state.reserve0 < U256::from(1000u64);
                    let r1_tiny =
                        state.reserve1 > U256::ZERO && state.reserve1 < U256::from(1000u64);
                    r0_tiny || r1_tiny
                })
                .count();
            if tiny_reserves > 0 {
                warn!(
                    "🔴 [BOOTSTRAP] {} pools com reserves <1000 (possível erro de decode)",
                    tiny_reserves
                );
            }

            // 🚨 CORREÇÃO: Mostrar estado do grafo após bootstrap
            info!(
                "[GRAPH] Estado inicial — {} pools com reserves",
                pools_with_reserves
            );
        }
        Err(e) => {
            error!("❌ [BOOTSTRAP] Falha no bootstrap simple: {}", e);
            warn!("⚠️ [BOOTSTRAP] Continuando sem inicialização de reserves");
        }
    }

    // ── LIGAÇÃO AO VIVO ÀS POOLS DESCOBERTAS ──
    // CORREÇÃO CRÍTICA: até aqui, só as ~90 pools fixas do PoolRegistry
    // (provider.rs) recebiam eventos Sync em tempo real. As 5000+ pools
    // descobertas via discovery/Multicall ficavam congeladas no snapshot do
    // arranque para sempre -- explica oportunidades que parecem boas mas
    // nunca sobrevivem até à execução (dados cada vez mais velhos).
    {
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
                    tokio::time::sleep(tokio::time::Duration::from_secs(600)).await;
                }
            });
        }
    }

    // Background scanning apos sincrono
    discovery_engine.start().await;

    // 🚀🚀🚀 SISTEMA OPERACIONAL - Ready for real-time monitoring 🚀🚀🚀
    info!("═══════════════════════════════════════════════════════════");
    info!("🎯 APEX-ENGINE OPERACIONAL - failed_state_speculator ATIVO");
    info!("🎯 Newton-Raphson precision: online | Shadow Hunter: armed");
    info!("🎯 Monitoring {} pools for MEV opportunities", pool_count);
    info!("═══════════════════════════════════════════════════════════");

    // TELEMETRIA - depois do discovery
    let telemetry_collector = Arc::new(TelemetryCollector::new());
    tokio::spawn(spawn_telemetry_printer(telemetry_collector.clone()));

    // � TELEMETRIA REAL - Benchmarking de hardware e latência
    info!("📊 TELEMETRIA REAL ATIVADA - Logs a cada 5s");

    // MempoolCollector
    let mev_broadcaster = Arc::new(orca_mev::executor::MevShareBroadcaster::new(
        provider.inner().clone(),
    ));
    let apex_protocol = ApexShadowProtocol::new(provider.inner().clone(), mev_broadcaster);
    apex_protocol.spawn().await?;

    let collector_config = CollectorConfigV2 {
        wss_urls: app_config.rpc_wss_urls.clone(),
        rpc_urls: app_config.rpc_http_urls.clone(),
        idle_timeout_ms: 5000,
        ping_interval_sec: 15,
        channel_capacity: 10000,
        max_reconnects_per_min: 10,
        debug_mode: false,
    };

    // MODO PROMÍSCUO TOTAL: Não filtrar por pools específicas
    // Receber TODOS os swaps de TODAS as pools na rede Base
    // 📍 OBTER POOLS DESCOBERTAS
    let discovered_pools = discovery_engine.get_all_pool_addresses().await;
    info!(
        "[SYSTEM] {} pools carregadas para o Collector",
        discovered_pools.len()
    );

    let log_filter = EventFilter {
        topic0s: vec![
            orca_mev::contracts::SYNC_TOPIC0.into(),
            orca_mev::contracts::SWAP_V3_TOPIC0.into(),
            orca_mev::contracts::SWAP_AERO_TOPIC0.into(),
        ],
        target_pools: discovered_pools,
        enabled_dexes: vec![DexType::UniswapV3, DexType::Aerodrome, DexType::PancakeSwap, DexType::UniswapV2],
        from_block: 0,
    };

    // 🚨 CORREÇÃO: Criar collector e receber o event_receiver (canal não dropado)
    let (collector, _event_rx) = LogCollectorV2::new(
        provider.inner().clone(),
        provider.inner().clone(), // Usar o mesmo para HTTP por agora
        log_filter,
        collector_config,
    )
    .await?;
    let collector = Arc::new(collector);

    info!("✅ [CANAL] Event receiver criado com capacidade 100_000 - canal ABERTO");

    let strategist_config = StrategistConfig {
        max_path_length: config.max_path_length,
        min_profit_bps: 10, // Reduzido para 10 bps (0.1%) para capturar oportunidades pequenas
        max_gas_price_gwei: 500,
        update_batch_size: 1000,
    };

    let profit_config = ProfitConfig {
        flash_loan_fee_bps: 30, // 0.3% Uniswap V3
        gas_price_gwei: 1,      // Base
        safety_margin_bps: 100, // 10%
        max_iterations: 10,
    };

    // ⚠️ VERIFICAÇÃO DE GAS NO ARRANQUE
    info!("═══════════════════════════════════════════════════════════");
    info!("⛽ GAS CONFIGURATION CHECK:");
    info!(
        "   Gas Price Configurado: {} gwei",
        profit_config.gas_price_gwei
    );
    info!(
        "   Max Gas Price: {} gwei",
        strategist_config.max_gas_price_gwei
    );
    info!("   Base Gas Cost: 21000 wei (hardcoded)");
    info!("   Gas per Hop: 100000 wei");
    info!("═══════════════════════════════════════════════════════════");
    info!("");
    info!("🔍 CONFIGURAÇÃO DO LOG COLLECTOR:");
    info!("   DEXs habilitadas: UniswapV3, Aerodrome, PancakeSwap");
    info!("   Pools: MODO PROMÍSCUO TOTAL (todas as pools da Base)");
    info!("   Buffer: 10.000 eventos");
    info!("   Modo: WebSocket (WSS) - Sem IPC");
    info!("");

    let executor_address = app_config.executor_address();

    let strategist =
        HighPerformanceStrategist::new(strategist_config, executor_address, profit_config);

    let min_profit_eth_wei = if app_config.dry_run {
        50_000_000_000_000u128 // 0.00005 ETH — diagnóstico em DRY_RUN
    } else {
        2_000_000_000_000_000u128 // 0.002 ETH — modo live
    };

    let strategy_context = StrategyContext {
        executor_address,
        max_gas_price: 100_000_000_000, // 100 gwei
        max_slippage_bps: 100,          // 1% slippage máximo
        min_profit_eth: min_profit_eth_wei,
        eth_price_usd: 3500.0,     // Preço ETH ~$3500
        priority_fee_gwei: 1,      // 1 gwei tip
        max_priority_fee_gwei: 50, // PILLAR 2: Hard-cap de 50 gwei
        max_reaction_time_ms: 100, // PILLAR 3: 100ms para reação
        dry_run: app_config.dry_run,
    };

    // ═══════════════════════════════════════════════════════════
    // 🦁 APEX-PREDATOR ENGINE - MOTOR DE LATÊNCIA ULTRA-BAIXA
    // ═══════════════════════════════════════════════════════════
    let apex_engine = Arc::new(ApexPredatorEngine::new(ApexConfig {
        max_cycle_hops: 5,
        min_cycle_hops: 3,
        liquidation_threshold_eth: 1.0,
        max_base_fee_gwei: 100,
        priority_tip_multiplier: 1.5,
        max_parallel_simulations: 20,
        cycle_reaction_time_us: 500,
    }));

    info!("StrategyEngine: Ciclo Multi-Hop ativo");

    // ═══════════════════════════════════════════════════════════
    // 🏛️ EMPIRE FOUNDATION ENGINE - VANTAGEM MATEMÁTICA ESTRUTURAL
    // ═══════════════════════════════════════════════════════════
    let empire_engine = Arc::new(EmpireFoundationEngine::new());

    // Mostrar benchmarks de gás
    let yul_benchmarks = empire_engine.yul_optimizer.benchmark_all();
    for benchmark in &yul_benchmarks {
        info!(
            "[YUL] {}: {} -> {} ({:.1}%)",
            benchmark.operation,
            benchmark.standard_gas,
            benchmark.optimized_gas,
            benchmark.savings_percent
        );
    }

    info!("YulOptimizer ativo");

    // ═══════════════════════════════════════════════════════════
    // 🌌 SINGULARIDADE MEV - META-CONSCIÊNCIA BLOCKCHAIN
    // ═══════════════════════════════════════════════════════════
    let singularity = Arc::new(SingularityMEV::new().await);

    // Iniciar monitorização de heartbeat
    singularity.sequencer_monitor.start_monitoring().await;

    // NOTA: Prober removido temporariamente (código incompleto)
    // singularity.prober.start_continuous_probing().await;
    // let rankings = singularity.prober.node_rankings().await;

    info!("SingularityMEV: Monitoramento ativo (sem prober)");

    // ═══════════════════════════════════════════════════════════
    // 🐋 ORCA ENGINE - PREDADOR DE ELITE (PASSIVE_OBSERVER)
    // ═══════════════════════════════════════════════════════════
    let eth_price_feed = orca_mev::pricing::EthPriceFeed::new();
    let mut orca_config = orca_mev::orca::OrcaConfig::default();
    orca_config.dry_run = app_config.dry_run;
    let mut orca_engine = orca_mev::orca::OrcaEngine::new(orca_config, discovery_engine.clone(), eth_price_feed.clone()).await;
    orca_engine.spawn_block_poller();
    orca_engine.set_shared_pool_cache((*pool_cache).clone());
    let shared_liquidity_count = pool_cache.count_pools_with_reserves();
    info!(
        "[VALIDATION] Cache partilhado tem {} pools com reserves",
        shared_liquidity_count
    );

    // 📊 LIGAR TELEMETRIA AO MOTOR ORCA — métricas em tempo real
    orca_engine.set_telemetry(telemetry_collector.clone());

    let orca_for_shutdown = orca_engine.clone();
    let orca_for_watchdog = orca_engine.clone();

    let engine = Arc::new(ArtemisEngine::new(
        collector.clone(),
        orca_engine,
        strategy_context,
    ));

    // Configurar sinal de paragem para relatório final
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Falha ao ouvir Ctrl+C");
        orca_for_shutdown.shutdown().await;
        std::process::exit(0);
    });

    // Iniciar monitorização em tempo real diretamente
    info!("Iniciando monitorização em tempo real...");

    // 🌪️ Iniciar simulação paralela Apex-Predator em background
    let apex_for_spawn = apex_engine.clone();
    let executor_for_apex = executor_address;
    tokio::spawn(async move {
        loop {
            // Monitorar ciclos a cada 100ms
            let _cycles = apex_for_spawn.hunt_cycles(executor_for_apex).await;
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    });

    // Spawn collector em background (spawn() é bloqueante até shutdown)
    let collector_for_task = collector.clone();
    tokio::spawn(async move {
        if let Err(err) = collector_for_task.spawn().await {
            error!("Collector V2 terminou com erro: {}", err);
        }
    });

    // 🚨 CORREÇÃO: ArtemisEngine workers vão processar eventos via subscribe_events()
    // NÃO criar outro consumer aqui — isso causaria "channel lagged"

    // Run engine


    // ── Heartbeat Discord a cada hora ──
    {
        let discord_hb = Arc::new(orca_mev::notifications::DiscordNotifier::new(
            &std::env::var("DISCORD_WEBHOOK").unwrap_or_default(),
            eth_price_feed.clone()
        ));
        let pool_cache_hb = pool_cache.clone();
        // CORRECAO: ler execucoes REAIS confirmadas (logs/executions.csv), nao
        // oportunidades detectadas (logs/opportunities.csv) -- esse CSV regista
        // TODAS as deteccoes (milhares/hora), nunca executadas na maioria, o que
        // produzia numeros impossiveis no Discord (ex: 36947/h execucoes).
        let exec_log_path = "logs/executions.csv".to_string();
        tokio::spawn(async move {
            let mut last_opps = 0u64;
            let mut last_profit = 0.0f64;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                let content = tokio::fs::read_to_string(&exec_log_path).await.unwrap_or_default();
                let lines: Vec<&str> = content.lines().skip(1).collect();
                let total_opps = lines.len() as u64;
                let total_profit: f64 = lines.iter()
                    .filter_map(|l| l.split(',').nth(2).and_then(|v| v.parse::<f64>().ok()))
                    .sum::<f64>() * 1800.0; // profit_eth -> EUR (preco fixo aqui; Discord usa preco ao vivo)
                let hour_opps = total_opps.saturating_sub(last_opps);
                let hour_profit = total_profit - last_profit;
                let block = pool_cache_hb.len() as u64;
                discord_hb.notify_heartbeat(block, hour_opps, hour_profit).await;
                last_opps = total_opps;
                last_profit = total_profit;
                // Verificar horário de paragem
                use chrono::{Local, Timelike};
                let now = Local::now();
                if now.hour() * 60 + now.minute() >= 21 * 60 + 30 {
                    discord_hb.notify_stop(total_opps, total_profit, 0.0).await;
                    break;
                }
            }
        });
    }

    {
        let watchdog_engine = orca_for_watchdog;
        tokio::spawn(async move {
            const WATCHDOG_TIMEOUT_MS: u64 = 180_000;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let last = watchdog_engine.last_event_ms();
                let stale_ms = now_ms.saturating_sub(last);
                if stale_ms > WATCHDOG_TIMEOUT_MS {
                    error!("💀 [WATCHDOG] {}ms sem eventos novos (limite {}ms) -- WebSocket morto, forcando saida do processo para restart externo", stale_ms, WATCHDOG_TIMEOUT_MS);
                    std::process::exit(137);
                }
            }
        });
    }
    info!("🚀 Artemis + Apex-Predator engines running - DOMINATING Base Mainnet...");
    engine.run().await.map_err(|e| {
        error!("Engine failure: {}", e);
        e
    })
}
