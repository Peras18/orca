pub mod collector;
pub mod collector_v2;
pub mod engine;
pub mod strategy;
pub mod apex_predator;
pub mod mempool_sniffer;

pub use collector::{LogCollector, LogFilter, CollectorConfig};
pub use collector_v2::{LogCollectorV2, CollectorConfigV2, EventFilter, CollectorMetrics};
pub use mempool_sniffer::{
    MempoolSniffer, SniffedTransaction, DecodedRoute, SnifferStats,
    KNOWN_MEV_BOTS, RouteStats,
};
pub use engine::ArtemisEngine;
pub use strategy::{Strategy, StrategyContext};
pub use apex_predator::{ApexPredatorEngine, ApexConfig, ApexOpportunityType, ApexStats};

use crate::contracts::NormalizedSwapEvent;

/// Evento processado pelo Artemis com metadados de latência
#[derive(Clone, Debug)]
pub enum MevEvent {
    Swap(NormalizedSwapEvent),
    BlockUpdate(u64),
    PriceUpdate { token: alloy::primitives::Address, price: f64 },
}

/// Metadados de latência para filtrar eventos stale
#[derive(Clone, Debug)]
pub struct EventMetadata {
    /// Timestamp quando o evento foi recebido (ms desde epoch)
    pub received_at_ms: u64,
    /// Timestamp do bloco/evento na blockchain (ms desde epoch)
    pub block_timestamp_ms: u64,
    /// Latência calculada (ms)
    pub latency_ms: u64,
}

impl EventMetadata {
    /// Cria metadados com timestamp atual
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        Self {
            received_at_ms: now_ms,
            block_timestamp_ms: now_ms, // Será atualizado se disponível
            latency_ms: 0,
        }
    }
    
    /// Verifica se o evento é stale (>500ms de atraso)
    pub fn is_stale(&self, max_latency_ms: u64) -> bool {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let current_latency = now_ms.saturating_sub(self.received_at_ms);
        current_latency > max_latency_ms
    }
}

/// Canal de eventos de alta performance
#[allow(dead_code)]
type EventTx = crossbeam::channel::Sender<MevEvent>;
#[allow(dead_code)]
type EventRx = crossbeam::channel::Receiver<MevEvent>;

/// Filtro de latência - descarta eventos antigos
pub struct LatencyFilter {
    pub max_latency_ms: u64,
}

impl Default for LatencyFilter {
    fn default() -> Self {
        Self { max_latency_ms: 2000 } // TEMP: 2000ms para debug (era 500ms)
    }
}

impl LatencyFilter {
    /// Verifica se o evento deve ser processado ou é "fantasma"
    pub fn should_process(&self, metadata: &EventMetadata) -> bool {
        if metadata.is_stale(self.max_latency_ms) {
            tracing::warn!("🚫 Evento STALE ignorado: {}ms > {}ms limite", 
                metadata.latency_ms, self.max_latency_ms);
            false
        } else {
            true
        }
    }
}
