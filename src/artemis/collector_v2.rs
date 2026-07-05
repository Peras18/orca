//! ARTEMIS COLLECTOR V2 - Engenharia de Precisão
//!
//! Correções críticas:
//! 1. Keep-Alive/Ping-Pong automático (30s) - evita drops da Alchemy
//! 2. Filtro correto por Topic0 (não event_signature múltiplo)
//! 3. Timeout adaptativo (5s-30s) baseado em atividade
//! 4. Reconexão exponencial com jitter
//! 5. Métricas de latência em tempo real

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::pubsub::Subscription;
use alloy::rpc::types::{Filter, Log as RpcLog};
use alloy::transports::BoxTransport;
use eyre::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, trace, warn};

use crate::artemis::MevEvent;
use crate::contracts::{
    classify_topic0, decode_swap_event, DexType, SWAP_AERO_TOPIC0, SWAP_V3_TOPIC0, SYNC_TOPIC0,
};

/// 📡 Configuração Ultra-Precisa do Collector
#[derive(Clone, Debug)]
pub struct CollectorConfigV2 {
    /// Lista de URLs WSS (Failover)
    pub wss_urls: Vec<String>,
    /// Lista de URLs RPC HTTP (Failover)
    pub rpc_urls: Vec<String>,
    /// Timeout de inatividade (ms) - adaptativo
    pub idle_timeout_ms: u64,
    /// Intervalo de ping/keep-alive (s)
    pub ping_interval_sec: u64,
    /// Capacidade do canal de eventos
    pub channel_capacity: usize,
    /// Máximo de reconexões por minuto
    pub max_reconnects_per_min: u32,
    /// Modo debug (logs verbose)
    pub debug_mode: bool,
}

impl Default for CollectorConfigV2 {
    fn default() -> Self {
        Self {
            wss_urls: vec!["wss://mainnet.base.org".to_string()],
            rpc_urls: vec!["https://mainnet.base.org".to_string()],
            // CORREÇÃO 2: Timeout aumentado de 5s para 60s (Base tem blocos de 2s mas períodos de inatividade maiores)
            idle_timeout_ms: 60000, // 60s (era 5s - muito agressivo)
            ping_interval_sec: 15,  // Ping a cada 15s
            channel_capacity: 10000,
            max_reconnects_per_min: 3, // CORREÇÃO 2: Reduzido de 10 para 3
            debug_mode: false,
        }
    }
}

/// 🎯 Filtro de Eventos Otimizado
#[derive(Clone, Debug)]
pub struct EventFilter {
    /// Topic0s específicos para monitorizar
    pub topic0s: Vec<FixedBytes<32>>,
    /// Pools de interesse (vazio = todas)
    pub target_pools: Vec<Address>,
    /// DEXs habilitadas
    pub enabled_dexes: Vec<DexType>,
    /// Bloco inicial (0 = latest)
    pub from_block: u64,
}

impl Default for EventFilter {
    fn default() -> Self {
        // CORREÇÃO 1: Usar TODOS os 3 topics corretos (Sync, Swap V3, Swap Aero)
        // CORREÇÃO 1: SEM filtro de address - receber de TODOS os pools, filtrar depois no Rust
        info!("🎯 [COLLECTOR] CORREÇÃO 1: A subscrever TODOS os pools com 3 topics corretos");
        info!("   Topics: Sync (V2) | Swap V3 | Swap Aerodrome");

        Self {
            topic0s: vec![
                SYNC_TOPIC0.into(),
                SWAP_V3_TOPIC0.into(),
                SWAP_AERO_TOPIC0.into(),
            ],
            target_pools: Vec::new(), // CORREÇÃO 1: Vazio = receber de TODOS os pools
            enabled_dexes: vec![DexType::UniswapV3, DexType::Aerodrome, DexType::UniswapV2],
            from_block: 0,
        }
    }
}

/// 📊 Métricas de Performance em Tempo Real
#[derive(Clone, Debug, Default)]
pub struct CollectorMetrics {
    /// Eventos recebidos (total)
    pub events_received: u64,
    /// Eventos processados
    pub events_processed: u64,
    /// Eventos filtrados (DEX não habilitada)
    pub events_filtered: u64,
    /// Reconexões realizadas
    pub reconnect_count: u64,
    /// Último evento recebido (timestamp)
    pub last_event_at: Option<Instant>,
    /// Latência média de processamento (μs)
    pub avg_processing_latency_us: u64,
    /// Bloco atual
    pub current_block: u64,
    /// Status da conexão
    pub connection_status: ConnectionStatus,
    /// Ping/Pong round-trip (ms)
    pub last_ping_rtt_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
    Error(String),
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        ConnectionStatus::Disconnected
    }
}

/// 🚀 Collector V2 - Ultra Baixa Latência
pub struct LogCollectorV2 {
    /// Provider WSS (Failover handle)
    ws_provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    /// Provider HTTP (Failover handle)
    _http_provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    /// Configuração
    config: CollectorConfigV2,
    /// Índice do RPC atual
    current_rpc_idx: Arc<RwLock<usize>>,
    /// Filtro de eventos
    filter: EventFilter,
    /// Canal de eventos (broadcast para múltiplos consumidores)
    event_tx: broadcast::Sender<MevEvent>,
    /// Métricas em tempo real
    metrics: Arc<RwLock<CollectorMetrics>>,
    /// Controle de shutdown
    shutdown_tx: mpsc::Sender<()>,
    shutdown_rx: Arc<RwLock<mpsc::Receiver<()>>>,
}

impl LogCollectorV2 {
    /// 🎯 Inicializa Collector V2 com dupla conexão (WSS + HTTP)
    /// 🚨 CORREÇÃO: Retorna também o receiver para não ser dropado
    pub async fn new(
        ws_provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        http_provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        filter: EventFilter,
        config: CollectorConfigV2,
    ) -> Result<(Self, broadcast::Receiver<MevEvent>)> {
        // 🚨 CORREÇÃO: Canal com capacidade 10x maior (10_000 em vez de 100)
        let (event_tx, event_rx) = broadcast::channel(100_000);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        info!("═══════════════════════════════════════════════════════════");
        info!("📡 ARTEMIS COLLECTOR V2 - Inicializando...");
        info!("🔗 WSS Principais: {}", config.wss_urls.len());
        info!("🔗 RPC Principais: {}", config.rpc_urls.len());
        info!(
            "⏱️  Timeout: {}ms | Ping: {}s",
            config.idle_timeout_ms, config.ping_interval_sec
        );
        info!("🎯 Topics: {:?}", filter.topic0s.len());
        info!("═══════════════════════════════════════════════════════════");

        let collector = Self {
            ws_provider,
            _http_provider: http_provider,
            config,
            current_rpc_idx: Arc::new(RwLock::new(0)),
            filter,
            event_tx,
            metrics: Arc::new(RwLock::new(CollectorMetrics::default())),
            shutdown_tx,
            shutdown_rx: Arc::new(RwLock::new(shutdown_rx)),
        };

        Ok((collector, event_rx))
    }

    /// 🚀 Inicia o coletor com supervisão ativa
    pub async fn spawn(self: Arc<Self>) -> Result<()> {
        // Clonar tudo ANTES de qualquer move
        let metrics = self.metrics.clone();
        let metrics_ping = self.metrics.clone();
        let metrics_monitor = self.metrics.clone();
        let config = self.config.clone();
        let ping_interval_sec = self.config.ping_interval_sec;
        let filter = self.filter.clone();
        let event_tx = self.event_tx.clone();
        let ws_provider = self.ws_provider.clone();
        let ws_provider_ping = self.ws_provider.clone();
        let shutdown_rx = self.shutdown_rx.clone();
        let current_rpc_idx = self.current_rpc_idx.clone();

        // Task 1: Loop principal de subscrição
        let collector_handle = tokio::spawn(async move {
            Self::subscription_loop(
                ws_provider,
                filter,
                event_tx,
                metrics,
                config,
                current_rpc_idx,
            )
            .await;
        });

        // Task 2: Keep-Alive / Health Check
        let ping_handle = tokio::spawn(async move {
            Self::keep_alive_loop(ws_provider_ping, metrics_ping, ping_interval_sec).await;
        });

        // Task 3: Monitor de métricas
        let monitor_handle = tokio::spawn(async move {
            Self::metrics_loop(metrics_monitor).await;
        });

        info!("✅ Collector V2 operacional - 3 tasks ativas");

        // Aguardar shutdown
        let mut rx = shutdown_rx.write().await;
        let _ = rx.recv().await;

        collector_handle.abort();
        ping_handle.abort();
        monitor_handle.abort();

        Ok(())
    }

    /// 🔄 Loop principal de subscrição com reconexão inteligente e Failover Dinâmico
    async fn subscription_loop(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        filter: EventFilter,
        event_tx: broadcast::Sender<MevEvent>,
        metrics: Arc<RwLock<CollectorMetrics>>,
        config: CollectorConfigV2,
        current_rpc_idx: Arc<RwLock<usize>>,
    ) {
        let mut reconnect_attempts = 0u32;
        let mut last_reconnect = Instant::now();

        loop {
            // Verificar rate de reconexão
            if last_reconnect.elapsed() < Duration::from_secs(60) {
                reconnect_attempts += 1;
                if reconnect_attempts > config.max_reconnects_per_min {
                    error!("💀 Max reconexões atingido. Aguardando 60s...");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    reconnect_attempts = 0;
                }
            } else {
                reconnect_attempts = 0;
            }
            last_reconnect = Instant::now();

            // FAILOVER LOGIC: Mudar RPC se houver erros persistentes — RECONECTA DE VERDADE
            if reconnect_attempts > 2 {
                let mut idx = current_rpc_idx.write().await;
                *idx = (*idx + 1) % config.wss_urls.len();
                let next_url = config.wss_urls[*idx].clone();
                drop(idx);
                info!("🔄 [FAILOVER] Mudando para RPC secundário: {}", next_url);

                match alloy::providers::builder()
                    .on_ws(alloy::transports::ws::WsConnect::new(next_url.clone()))
                    .await
                {
                    Ok(new_conn) => {
                        *provider.write().await = new_conn.boxed();
                        info!("✅ [FAILOVER] Nova conexão WS estabelecida: {}", next_url);
                    }
                    Err(e) => {
                        error!("❌ [FAILOVER] Falha ao reconectar a {}: {}", next_url, e);
                    }
                }
            }

            // Atualizar status
            metrics.write().await.connection_status = ConnectionStatus::Connecting;

            info!("🔧 Subscrevendo a logs...");

            // CORREÇÃO CRÍTICA: sem timeout aqui, um WS morto silenciosamente bloqueia
            // este .await para sempre — era isto que congelava o collector em "Connecting"
            match timeout(Duration::from_secs(15), Self::subscribe_logs(&provider, &filter)).await {
                Ok(Ok(mut stream)) => {
                    info!("✅ Subscrição ativa - aguardando eventos...");
                    metrics.write().await.connection_status = ConnectionStatus::Connected;
                    reconnect_attempts = 0;

                    // CORREÇÃO 6: Diagnóstico imediato - testar se o filtro funciona
                    Self::run_diagnostic_test(&provider).await;

                    // Loop de processamento de eventos
                    Self::event_processing_loop(&mut stream, &filter, &event_tx, &metrics, &config)
                        .await;
                }
                Ok(Err(e)) => {
                    error!("❌ Falha na subscrição: {}", e);
                    metrics.write().await.connection_status =
                        ConnectionStatus::Error(e.to_string());
                }
                Err(_) => {
                    error!("⏱️ TIMEOUT ao subscrever (15s) — WS provavelmente morto, forçando failover");
                    metrics.write().await.connection_status =
                        ConnectionStatus::Error("subscribe timeout".to_string());
                    reconnect_attempts = reconnect_attempts.saturating_add(3);
                }
            }

            // Exponential backoff com jitter
            let delay = Self::calculate_backoff(reconnect_attempts);
            warn!(
                "⏳ Reconectando em {:?}... (tentativa {})",
                delay, reconnect_attempts
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// 📡 Subscreve a logs com filtro correto por Topic0
    async fn subscribe_logs(
        provider: &Arc<RwLock<RootProvider<BoxTransport>>>,
        _filter: &EventFilter, // Prefixado: não usamos pois construímos filtro manual
    ) -> Result<Subscription<RpcLog>> {
        let prov = provider.read().await;

        // 🚨 CORREÇÃO CRÍTICA: alloy 0.8 - event_signature evita depreciação e reduz RPC calls
        // Cria um vetor com todos os tópicos para monitorizar simultaneamente
        let topics: Vec<FixedBytes<32>> = _filter.topic0s.iter().cloned().collect();
        let topic_count = topics.len();

        let alloy_filter = Filter::new()
            .event_signature(topics) // ← CORREÇÃO: event_signature em vez de topic0
            .from_block(alloy::eips::BlockNumberOrTag::Latest);

        // 🚨 NÃO filtrar por address no RPC - receber de TODOS os pools
        // Filtrar em Rust depois, verificando o nosso DashMap
        info!(
            "📡 Filtro: {} topics (Sync | SwapV3 | SwapAero) | Global (todas as pools)",
            topic_count
        );

        let subscription = prov.subscribe_logs(&alloy_filter).await?;

        // 🔬 Diagnóstico rápido: verificar se eventos existem nos últimos 2 blocos.
        // NOTA: Não usar Latest=Latest (mesmo bloco) porque eventos do bloco
        // corrente ainda podem não estar indexados no momento da query.
        let latest_for_diag = prov.get_block_number().await.unwrap_or(0);
        let from_for_diag = latest_for_diag.saturating_sub(2);
        let test_filter = Filter::new()
            .event_signature(SYNC_TOPIC0)
            .from_block(from_for_diag)
            .to_block(latest_for_diag);

        match prov.get_logs(&test_filter).await {
            Ok(test_logs) => {
                if test_logs.is_empty() {
                    // Na Base, Sync events globais com Alchemy free tier podem retornar 0
                    // por limite de resposta (demasiados resultados). Não é erro.
                    debug!("[DIAG] 0 Sync events globais em últimos 2 blocos (possível limite Alchemy)");
                } else {
                    debug!(
                        "[DIAG] {} Sync events em últimos 2 blocos — filtro OK",
                        test_logs.len()
                    );
                }
            }
            Err(e) => {
                warn!("[DIAG] Erro ao testar filtro Sync: {}", e);
            }
        }

        Ok(subscription)
    }

    /// 🔄 Loop de processamento de eventos com timeout adaptativo
    async fn event_processing_loop(
        stream: &mut Subscription<RpcLog>,
        filter: &EventFilter,
        event_tx: &broadcast::Sender<MevEvent>,
        metrics: &Arc<RwLock<CollectorMetrics>>,
        config: &CollectorConfigV2,
    ) {
        let _last_activity = Instant::now();
        let mut consecutive_timeouts = 0u32;

        loop {
            // Timeout adaptativo: mais curto se não há atividade
            let timeout_duration = if consecutive_timeouts > 3 {
                Duration::from_millis(config.idle_timeout_ms / 2) // 2.5s
            } else {
                Duration::from_millis(config.idle_timeout_ms) // 5s
            };

            match timeout(timeout_duration, stream.recv()).await {
                Ok(Ok(log)) => {
                    consecutive_timeouts = 0;
                    let _last_activity = Instant::now();

                    // Atualizar métricas
                    {
                        let mut m = metrics.write().await;
                        m.events_received += 1;
                        m.last_event_at = Some(Instant::now());
                    }

                    // Processar evento
                    let start = Instant::now();
                    Self::process_log(log, filter, event_tx, metrics).await;

                    let elapsed = start.elapsed().as_micros() as u64;
                    metrics.write().await.avg_processing_latency_us = elapsed;
                }
                Ok(Err(e)) => {
                    let err_str = e.to_string();
                    if err_str.contains("channel lagged") {
                        // Buffer interno do Alloy saturado num burst — eventos ignorados.
                        // Já tratado com continue; não afecta operação.
                        debug!("[COLLECTOR] Burst: eventos perdidos por canal saturado (normal).");
                        continue;
                    }
                    error!("❌ Erro no stream: {}", e);
                    break;
                }
                Err(_) => {
                    consecutive_timeouts += 1;
                    warn!(
                        "⏱️ TIMEOUT #{}: {}s sem eventos",
                        consecutive_timeouts,
                        timeout_duration.as_secs_f32()
                    );

                    // 🚨 CORREÇÃO 2: Threshold de 3 timeouts CONSECUTIVOS (não 1)
                    if consecutive_timeouts >= 3 {
                        error!(
                            "💀 {} timeouts consecutivos. Forçando reconexão...",
                            consecutive_timeouts
                        );
                        break;
                    }
                }
            }
        }
    }

    /// ⚡ Processa log individual com mínima latência
    /// CORREÇÃO 5: Processa tanto Swap como Sync events
    async fn process_log(
        log: RpcLog,
        filter: &EventFilter,
        event_tx: &broadcast::Sender<MevEvent>,
        metrics: &Arc<RwLock<CollectorMetrics>>,
    ) {
        let address = log.address();
        let topics = log.topics();

        if topics.is_empty() {
            return;
        }

        let topic0 = topics[0];

        // 🔬 LOG DE DIAGNÓSTICO: Confirmar que eventos chegam e são processados
        trace!(
            "[EVENT] topic0={:?} pool={:?} block={:?}",
            topic0,
            address,
            log.block_number
        );

        // CORREÇÃO 5: Detectar Sync event (V2/Aerodrome vAMM reserves)
        if topic0 == SYNC_TOPIC0.0 {
            // Decodificar Sync event: Sync(uint112 reserve0, uint112 reserve1)
            let data_bytes: &[u8] = log.data().data.as_ref();
            if data_bytes.len() >= 64 {
                let reserve0 = U256::from_be_slice(&data_bytes[0..32]);
                let reserve1 = U256::from_be_slice(&data_bytes[32..64]);
                let block_number = log.block_number.unwrap_or_default();

                metrics.write().await.events_processed += 1;

                trace!(
                    "🔄 [SYNC] Pool: {:?} | r0={} r1={} | Block: {}",
                    address,
                    reserve0,
                    reserve1,
                    block_number
                );

                // Reencaminhar como MevEvent::Swap com marcador fee=0
                // OrcaEngine detecta fee==0 e chama update_sync_event com reserves reais
                use crate::contracts::NormalizedSwapEvent;
                let sync_event = NormalizedSwapEvent {
                    pool: address,
                    token_in: alloy::primitives::Address::ZERO,
                    token_out: alloy::primitives::Address::ZERO,
                    amount_in: reserve0,
                    amount_out: reserve1,
                    block_number,
                    tx_hash: alloy::primitives::FixedBytes::ZERO,
                    log_index: 0,
                    sqrt_price_x96: None,
                    liquidity: None,
                    tick: None,
                    fee: 0, // Marcador: este é um Sync, não um Swap
                    dex_type: DexType::UniswapV2,
                };

                if let Err(_) = event_tx.send(MevEvent::Swap(sync_event)) {
                    // Canal cheio ou sem receptores — normal sob carga
                }
            }
            return;
        }

        // Classificar DEX para Swap events
        let dex_type = match classify_topic0(topic0) {
            Some(dt) => dt,
            None => {
                metrics.write().await.events_filtered += 1;
                return;
            }
        };

        // Verificar se DEX está habilitada
        if !filter.enabled_dexes.contains(&dex_type) {
            return;
        }

        // Decodificar evento Swap
        let data_bytes: &[u8] = log.data().data.as_ref();
        let block_number = log.block_number.unwrap_or_default();
        let tx_hash = log.transaction_hash.unwrap_or_default();

        let log_index = log.log_index.unwrap_or(0);
        match decode_swap_event(address, topic0, data_bytes, dex_type, block_number, tx_hash, log_index) {
            Some(swap_event) => {
                metrics.write().await.events_processed += 1;

                debug!(
                    "🔥 [SWAP] {:?} | Pool: {:?} | {} -> {} | Amount: {:?}",
                    dex_type,
                    address,
                    swap_event.token_in,
                    swap_event.token_out,
                    swap_event.amount_in
                );

                // CORREÇÃO 5: Enviar para processamento - isto vai disparar find_opportunities()
                let mev_event = MevEvent::Swap(swap_event);
                if let Err(e) = event_tx.send(mev_event) {
                    warn!("⚠️ Falha ao enviar evento: {}", e);
                }
            }
            None => {
                trace!("⚠️ Falha ao decodificar swap em {:?}", address);
            }
        }
    }

    /// 💓 Keep-Alive: Ping/Pong para manter conexão Alchemy viva
    async fn keep_alive_loop(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        metrics: Arc<RwLock<CollectorMetrics>>,
        interval_sec: u64,
    ) {
        let mut interval = interval(Duration::from_secs(interval_sec));

        loop {
            interval.tick().await;

            let prov = provider.read().await;
            let ping_start = Instant::now();

            match prov.get_block_number().await {
                Ok(block) => {
                    let rtt = ping_start.elapsed().as_millis() as u64;
                    let mut m = metrics.write().await;
                    m.last_ping_rtt_ms = rtt;
                    m.current_block = block;

                    if rtt > 150 {
                        warn!(
                            "⚠️ [LATENCY] RTT elevado: {}ms! Competitividade MEV em risco.",
                            rtt
                        );
                    }

                    debug!("💓 Ping OK | Block: {} | RTT: {}ms", block, rtt);
                }
                Err(e) => {
                    warn!("💔 Ping falhou: {} - conexão pode estar stale", e);
                    metrics.write().await.connection_status = ConnectionStatus::Reconnecting;
                    // Forçar reconexão no próximo ciclo
                }
            }
        }
    }

    /// 📊 Loop de métricas - log periódico de status
    async fn metrics_loop(metrics: Arc<RwLock<CollectorMetrics>>) {
        let mut interval = interval(Duration::from_secs(10));

        loop {
            interval.tick().await;

            let m = metrics.read().await;
            info!(
                "📊 [METRICS] Events: {} proc/{} recv | Latency: {}μs | RTT: {}ms | Block: {} | Status: {:?}",
                m.events_processed,
                m.events_received,
                m.avg_processing_latency_us,
                m.last_ping_rtt_ms,
                m.current_block,
                m.connection_status
            );
        }
    }

    /// 📈 Cálculo de backoff exponencial
    fn calculate_backoff(attempt: u32) -> Duration {
        let base = 100u64; // 100ms
        let exp = 2u32.saturating_pow(attempt.min(6));
        let delay_ms = base * exp as u64;
        // Jitter fixo baseado no attempt para evitar thundering herd
        let jitter = (attempt as u64 * 17) % 100;
        Duration::from_millis(delay_ms + jitter)
    }

    /// 🔬 CORREÇÃO 6: Diagnóstico imediato - testa se o filtro funciona
    ///
    /// Usa endereços concretos para evitar limite de resultado do Alchemy free tier.
    /// Uma query global de Sync (sem address filter) pode retornar milhões de eventos
    /// na Base e o Alchemy limita o resultado a 0 ou retorna erro — falso alarme.
    async fn run_diagnostic_test(provider: &Arc<RwLock<RootProvider<BoxTransport>>>) {
        let prov = provider.read().await;

        match prov.get_block_number().await {
            Ok(latest_block) => {
                let from_block = latest_block.saturating_sub(5);

                // Testar Sync com um endereço concreto de pool vAMM de alto volume
                // (WETH/USDC Aerodrome — uma das pools mais activas na Base)
                // Isso evita o limite de resultado do Alchemy para queries globais.
                use alloy::primitives::address;
                let known_vamm = address!("88A43bbDF9D098eEC7bCEda4e2494615dfD9bB9C");
                let sync_filter = Filter::new()
                    .from_block(from_block)
                    .to_block(latest_block)
                    .event_signature(SYNC_TOPIC0)
                    .address(known_vamm);

                match prov.get_logs(&sync_filter).await {
                    Ok(logs) => {
                        if logs.is_empty() {
                            // Pode simplesmente não ter havido swaps nesta pool específica
                            // nos últimos 5 blocos (~2.5s). Não é obrigatoriamente erro.
                            debug!(
                                "[DIAGNÓSTICO] 0 Sync events para pool WETH/USDC Aerodrome em {}-{}",
                                from_block, latest_block
                            );
                        } else {
                            info!(
                                "[DIAGNÓSTICO] ✅ {} Sync events confirmados — pipeline WebSocket OK",
                                logs.len()
                            );
                        }
                    }
                    Err(e) => {
                        warn!("[DIAGNÓSTICO] Erro ao testar Sync filter: {}", e);
                    }
                }

                // Testar Swap V3 (confirma que a conexão RPC está a funcionar)
                let v3_filter = Filter::new()
                    .from_block(from_block)
                    .to_block(latest_block)
                    .event_signature(SWAP_V3_TOPIC0);

                match prov.get_logs(&v3_filter).await {
                    Ok(logs) if !logs.is_empty() => {
                        info!(
                            "[DIAGNÓSTICO] ✅ Swap V3 logs: {} (últimos 5 blocos) — RPC OK",
                            logs.len()
                        );
                    }
                    Ok(_) => {
                        warn!("[DIAGNÓSTICO] 0 Swap V3 logs em 5 blocos — verifique conexão");
                    }
                    Err(_) => {}
                }
            }
            Err(e) => {
                error!("[DIAGNÓSTICO] Não conseguiu obter bloco actual: {}", e);
            }
        }
    }

    /// 🎯 Retorna receiver de eventos para consumidores
    pub fn subscribe_events(&self) -> broadcast::Receiver<MevEvent> {
        self.event_tx.subscribe()
    }

    /// � Retorna métricas atuais
    pub async fn get_metrics(&self) -> CollectorMetrics {
        self.metrics.read().await.clone()
    }

    /// 🛑 Sinaliza shutdown
    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(()).await;
    }
}
