#![allow(dead_code)]

use alloy::primitives::{Address, FixedBytes, B256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::eth::{Filter, Log};
use alloy::transports::BoxTransport;
use alloy::consensus::Transaction;
use crossbeam::channel::{bounded, Sender, Receiver};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, trace, warn};

use crate::contracts::{classify_topic0, decode_swap_event, NormalizedSwapEvent, DexType};
use super::MevEvent;

/// Tópicos de eventos de Swap para filtro Alchemy
pub const TOPICS_SWAP_EVENTS: [[u8; 32]; 2] = [
    // Uniswap V3 / PancakeSwap V3 Swap
    [
        0xc4, 0x20, 0x79, 0x94, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ],
    // Aerodrome / Uniswap V2 Swap
    [
        0x1c, 0x41, 0x11, 0x76, 0x09, 0x01, 0x69, 0x05,
        0x21, 0xbe, 0x8c, 0xbf, 0x56, 0xd1, 0xeb, 0xc0,
        0x9d, 0x09, 0x29, 0x61, 0x98, 0x2c, 0x30, 0x26,
        0xaf, 0x4f, 0xb1, 0xcb, 0x41, 0xd7, 0x49, 0x05,
    ],
];

/// Tópicos hardcoded para máxima compatibilidade
/// Uniswap V3 / PancakeSwap V3 Swap event: keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
pub const TOPIC_UNISWAP_V3_SWAP: [u8; 32] = [
    0xc4, 0x20, 0x21, 0xa1, 0xa4, 0xc4, 0x40, 0x33,
    0x10, 0x08, 0xf1, 0xb6, 0x37, 0x95, 0x32, 0x88,
    0x92, 0x1e, 0x25, 0xe8, 0xa7, 0x19, 0x9c, 0x0d,
    0x95, 0x95, 0x2c, 0x42, 0x17, 0x15, 0x89, 0x1a,
];

/// Aerodrome / Uniswap V2 Swap event: keccak256("Swap(address,uint256,uint256,uint256,uint256,address)")
pub const TOPIC_AERODROME_SWAP: [u8; 32] = [
    0xd7, 0x8a, 0xd9, 0x5f, 0xa4, 0x6c, 0x99, 0x4b,
    0x65, 0x51, 0xd0, 0xda, 0x85, 0xfc, 0x27, 0x5f,
    0xe6, 0x13, 0xce, 0x37, 0x65, 0x7f, 0xb8, 0xd5,
    0xe3, 0xd1, 0x30, 0x84, 0x01, 0x59, 0xd8, 0x22,
];

/// Configuração do coletor
#[derive(Clone, Debug)]
pub struct CollectorConfig {
    pub wss_url: String,
    pub max_concurrent_logs: usize,
    pub channel_capacity: usize,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            wss_url: std::env::var("BASE_WSS_URL")
                .unwrap_or_else(|_| "wss://base-mainnet.g.alchemy.com/v2/demo".to_string()),
            max_concurrent_logs: 10000,
            channel_capacity: 10000, // Buffer de 10.000 para evitar bloqueios
        }
    }
}

/// Filtro otimizado para eventos relevantes
#[derive(Clone, Debug)]
pub struct LogFilter {
    /// Pools monitorados (vazio = todos)
    pub target_pools: Vec<Address>,
    /// Tokens de interesse (vazio = todos)
    pub target_tokens: Vec<Address>,
    /// DEXs habilitadas
    pub enabled_dexes: Vec<DexType>,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            target_pools: Vec::new(),
            target_tokens: Vec::new(),
            enabled_dexes: vec![DexType::UniswapV3, DexType::Aerodrome],
        }
    }
}

impl LogFilter {
    /// Verifica se um log deve ser processado
    #[inline(always)]
    pub fn should_process(&self, address: Address, topic0: FixedBytes<32>) -> bool {
        // Verificar se é um evento de swap conhecido
        let dex_type = match classify_topic0(topic0) {
            Some(dt) => dt,
            None => return false,
        };

        // Verificar se a DEX está habilitada
        if !self.enabled_dexes.contains(&dex_type) {
            return false;
        }

        // Se temos pools alvo, verificar
        if !self.target_pools.is_empty() && !self.target_pools.contains(&address) {
            return false;
        }

        true
    }
}

/// Coletor de logs de ultra-baixa latência
pub struct LogCollector {
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    filter: LogFilter,
    event_tx: Sender<MevEvent>,
    event_rx: Receiver<MevEvent>,
    #[allow(dead_code)]
    config: CollectorConfig,
    /// Último timestamp de atividade (para heartbeat)
    last_activity: Arc<RwLock<std::time::Instant>>,
    /// Contador de swaps analisados (último minuto)
    swaps_count: Arc<RwLock<u64>>,
    /// Último bloco recebido
    last_block: Arc<RwLock<u64>>,
    /// Pools carregadas em memória para filtro O(1)
    known_pools: Arc<DashMap<Address, ()>>,
}

impl LogCollector {
    pub fn new(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        filter: LogFilter,
        config: CollectorConfig,
    ) -> Self {
        let (event_tx, event_rx) = bounded(config.channel_capacity);
        
        Self {
            known_pools: Arc::new(
                filter
                    .target_pools
                    .iter()
                    .copied()
                    .map(|addr| (addr, ()))
                    .collect()
            ),
            provider,
            filter,
            event_tx,
            event_rx,
            config,
            last_activity: Arc::new(RwLock::new(std::time::Instant::now())),
            swaps_count: Arc::new(RwLock::new(0)),
            last_block: Arc::new(RwLock::new(0)),
        }
    }

    /// Verifica se o URL é do Anvil (localhost/127.0.0.1)
    fn is_anvil_url(url: &str) -> bool {
        url.contains("127.0.0.1") || url.contains("localhost") || url.contains(":8545")
    }

    /// Verifica se o URL é da Alchemy (wss://)
    fn is_alchemy_url(url: &str) -> bool {
        url.starts_with("wss://") && url.contains("alchemy.com")
    }

    /// Inicia o coletor com deteção automática de modo (Anvil vs Alchemy)
    pub async fn spawn(self: Arc<Self>) -> eyre::Result<()> {
        let provider = self.provider.clone();
        let last_activity = self.last_activity.clone();
        let swaps_count = self.swaps_count.clone();
        let last_block = self.last_block.clone();
        let filter = self.filter.clone();
        let event_tx = self.event_tx.clone();
        let known_pools = self.known_pools.clone();
        let wss_url = self.config.wss_url.clone();
        
        // Detetar modo de operação
        let is_anvil = Self::is_anvil_url(&wss_url);
        let is_alchemy = Self::is_alchemy_url(&wss_url);
        
        tokio::spawn(async move {
            if is_anvil {
                // MODO ANVIL - Pending Transactions
                Self::run_anvil_mode(provider, last_activity, swaps_count, wss_url).await;
            } else if is_alchemy {
                // MODO ALCHEMY - Subscribe Logs (Eventos de Swap)
                Self::run_alchemy_mode(
                    provider,
                    filter,
                    event_tx,
                    known_pools,
                    last_activity,
                    swaps_count,
                    last_block,
                    wss_url,
                ).await;
            } else {
                // Modo genérico - tentar Alchemy
                warn!("⚠️ URL não reconhecido como Anvil ou Alchemy. Tentando modo Alchemy...");
                Self::run_alchemy_mode(
                    provider,
                    filter,
                    event_tx,
                    known_pools,
                    last_activity,
                    swaps_count,
                    last_block,
                    wss_url,
                ).await;
            }
        });

        Ok(())
    }

    /// Modo ANVIL: Subscreve a pending transactions
    async fn run_anvil_mode(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        last_activity: Arc<RwLock<std::time::Instant>>,
        swaps_count: Arc<RwLock<u64>>,
        wss_url: String,
    ) {
        info!("═══════════════════════════════════════════════════════════");
        info!("🚀 [LAB] MODO PREDADOR TOTAL ATIVADO");
        info!("   Ouvindo TODAS as transações pendentes do Anvil");
        info!("   URL: {}", wss_url);
        info!("═══════════════════════════════════════════════════════════");

        loop {
            let prov = provider.read().await;
            
            info!("🔧 Subscrevendo a pending transactions no Anvil...");
            match prov.subscribe_full_pending_transactions().await {
                Ok(mut stream) => {
                    info!("✅ Subscrição ativa - aguardando TXs...");
                    
                    let mut reconnect_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
                    
                    loop {
                        info!("🔥 [PREDADOR] Aguardando TX no mempool do ANVIL...");
                        
                        let timeout_remaining = reconnect_deadline.duration_since(tokio::time::Instant::now());
                        
                        match tokio::time::timeout(timeout_remaining, stream.recv()).await {
                            Ok(Ok(tx)) => {
                                use alloy::network::TransactionResponse;
                                *last_activity.write().await = std::time::Instant::now();
                                *swaps_count.write().await += 1;
                                reconnect_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
                                
                                let tx_hash = tx.tx_hash();
                                let from = tx.from();
                                let to = tx.to().unwrap_or_default();
                                info!("🔥 [PREDADOR] TX DETETADA! Hash: {:?} | From: {:?} | To: {:?}", tx_hash, from, to);
                                
                                info!("⚡ TRIGGER: Analisando TX para oportunidades MEV");
                            }
                            Ok(Err(e)) => {
                                error!("❌ Erro no stream: {}", e);
                                break;
                            }
                            Err(_) => {
                                warn!("⚠️ TIMEOUT: 30s sem TXs! Reconectando...");
                                break;
                            }
                        }
                    }
                    
                    warn!("⚠️ Stream fechado - reconectando...");
                }
                Err(e) => {
                    error!("❌ Falha na subscrição: {}. Tentando em 1s...", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    /// Modo ALCHEMY: Subscreve a eventos de Swap (logs)
    async fn run_alchemy_mode(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        log_filter: LogFilter,
        event_tx: Sender<MevEvent>,
        known_pools: Arc<DashMap<Address, ()>>,
        last_activity: Arc<RwLock<std::time::Instant>>,
        swaps_count: Arc<RwLock<u64>>,
        last_block: Arc<RwLock<u64>>,
        wss_url: String,
    ) {
        use alloy::primitives::FixedBytes;
        
        info!("═══════════════════════════════════════════════════════════");
        info!("🔗 [ALCHEMY] MODO MAINNET ATIVADO");
        info!("   Ouvindo eventos de Swap na Base Mainnet");
        info!("   URL: {}", wss_url.replace("wss://", "wss://***"));
        info!("═══════════════════════════════════════════════════════════");

        // Filtro global por topic0 (sem filtro de addresses na RPC).
        let rpc_filter = Filter::new()
            .event_signature(FixedBytes::new(TOPICS_SWAP_EVENTS[0])) // Uniswap V3 Swap
            .event_signature(FixedBytes::new(TOPICS_SWAP_EVENTS[1])); // Aerodrome Swap

        loop {
            let prov = provider.read().await;
            
            info!("🔧 Subscrevendo a eventos de Swap na Alchemy...");
            match prov.subscribe_logs(&rpc_filter).await {
                Ok(mut stream) => {
                    info!("✅ Subscrição ativa - aguardando Swaps...");
                    
                    let mut reconnect_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
                    
                    loop {
                        info!("🔥 [ALCHEMY] Aguardando eventos de SWAP...");
                        
                        let timeout_remaining = reconnect_deadline.duration_since(tokio::time::Instant::now());
                        
                        match tokio::time::timeout(timeout_remaining, stream.recv()).await {
                            Ok(Ok(log)) => {
                                *last_activity.write().await = std::time::Instant::now();
                                *swaps_count.write().await += 1;
                                reconnect_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
                                
                                let address = log.address();
                                let tx_hash = log.transaction_hash.unwrap_or_default();
                                
                                info!("🔥 [ALCHEMY] SWAP DETETADO! Pool: {:?} | TX: {:?}", address, tx_hash);

                                // Filtro em memória (DashMap O(1)): se vazio, modo promíscuo global.
                                if !known_pools.is_empty() && !known_pools.contains_key(&address) {
                                    trace!("[GLOBAL-SWAP] Pool fora da whitelist em memória: {:?}", address);
                                    continue;
                                }

                                if let Err(err) = Self::process_log(
                                    &log_filter,
                                    &event_tx,
                                    log,
                                    true,
                                    last_block.clone(),
                                ).await {
                                    error!("❌ Erro ao processar log global: {}", err);
                                }
                            }
                            Ok(Err(e)) => {
                                error!("❌ Erro no stream de logs: {}", e);
                                break;
                            }
                            Err(_) => {
                                warn!("⚠️ TIMEOUT: 60s sem eventos! Reconectando...");
                                break;
                            }
                        }
                    }
                    
                    warn!("⚠️ Stream de logs fechado - reconectando...");
                }
                Err(e) => {
                    error!("❌ Falha na subscrição de logs: {}. Tentando em 1s...", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    /// Processa um log individual de forma não-bloqueante
    #[inline(always)]
    async fn process_log(
        filter: &LogFilter,
        event_tx: &Sender<MevEvent>,
        log: Log,
        _is_open_pipe: bool,
        last_block: Arc<RwLock<u64>>,
    ) -> eyre::Result<()> {
        let address = log.address();
        
        // Atualizar último bloco
        if let Some(block_number) = log.block_number {
            *last_block.write().await = block_number;
        }
        
        // Extrair topic0
        let topics = log.topics();
        if topics.is_empty() {
            return Ok(());
        }
        let topic0 = FixedBytes::new(topics[0].0);

        // 🕵️ SHADOW SCANNING: Log de TODO evento swap na rede
        let dex_type = match classify_topic0(topic0) {
            Some(dt) => dt,
            None => {
                trace!("[SHADOW-SCAN] Evento em {:?} | Status: Ignorado (Topic não reconhecido)", address);
                return Ok(());
            }
        };

        // Verificar se a DEX está habilitada
        if !filter.enabled_dexes.contains(&dex_type) {
            trace!("[SHADOW-SCAN] Evento em {:?} | Status: Ignorado (DEX {:?} desabilitada)", address, dex_type);
            return Ok(());
        }

        // Decodificar evento de swap
        let data_bytes: &[u8] = log.data().data.as_ref();
        let block_number = log.block_number.unwrap_or_default();
        let tx_hash = log.transaction_hash.unwrap_or_default();
        let log_index = log.log_index.unwrap_or(0);

        match decode_swap_event(address, topic0, data_bytes, dex_type, block_number, tx_hash, log_index) {
            Some(swap_event) => {
                // 🎯 SHADOW SCAN: Pool catalogada - Iniciar simulação
                info!(
                    "[SHADOW-SCAN] Pool detetada | Iniciando Simulação Atómica... | {:?} | {:?} -> {:?}",
                    address,
                    swap_event.token_in,
                    swap_event.token_out
                );
                
                // Enviar evento para o engine
                let mev_event = MevEvent::Swap(swap_event);
                if let Err(e) = event_tx.try_send(mev_event) {
                    error!("❌ Falha ao enviar evento: {}", e);
                }
            }
            None => {
                trace!("[SHADOW-SCAN] Evento em {:?} | Status: Ignorado (Falha decodificação)", address);
            }
        }

        Ok(())
    }

    /// Retorna o receptor de eventos
    pub fn event_receiver(&self) -> Receiver<MevEvent> {
        self.event_rx.clone()
    }

    /// Busca logs históricos para inicialização rápida
    pub async fn fetch_historical_logs(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<Vec<NormalizedSwapEvent>> {
        let prov = self.provider.read().await;
        
        let filter = Filter::new()
            .from_block(from_block)
            .to_block(to_block)
            .event_signature(vec![
                B256::new(TOPIC_UNISWAP_V3_SWAP),
                B256::new(TOPIC_AERODROME_SWAP),
            ]);

        let logs = prov.get_logs(&filter).await?;
        
        let swaps = Vec::with_capacity(logs.len());
        
        // Processamento de logs históricos temporariamente simplificado
        let _ = logs;
        info!("Busca de logs históricos desabilitada temporariamente");
        Ok(swaps)
    }
}
