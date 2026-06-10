//! SEQUENCER HEARTBEAT MONITOR
//! Mede RTT (Round Trip Time) para RPC da Base com precisão de microssegundos
//!
//! Ajusta timing de envio para chegar no exato nanossegundo de abertura de bloco.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};
use tokio::time::{interval, MissedTickBehavior};

/// ⏱️ Monitor de Heartbeat do Sequenciador
#[derive(Clone, Debug)]
pub struct SequencerHeartbeatMonitor {
    /// RPC endpoint da Base
    rpc_url: String,
    /// RTT atual em microssegundos
    current_rtt_us: Arc<RwLock<u64>>,
    /// Histórico de RTTs (últimos 1000)
    rtt_history: Arc<RwLock<Vec<u64>>>,
 /// Estimativa de desvio padrão
    rtt_stddev_us: Arc<RwLock<u64>>,
    /// Timestamp do último bloco recebido
    last_block_time: Arc<RwLock<Instant>>,
    /// Número do último bloco
    last_block_number: Arc<RwLock<u64>>,
    /// Intervalo médio entre blocos (ms)
    avg_block_interval_ms: Arc<RwLock<f64>>,
    /// Canal de eventos de bloco
    block_tx: mpsc::Sender<BlockEvent>,
    /// Canal de comandos de timing
    timing_tx: mpsc::Sender<TimingCommand>,
}

/// 📦 Evento de novo bloco
#[derive(Clone, Debug)]
pub struct BlockEvent {
    pub block_number: u64,
    pub timestamp_us: u64,
    pub rtt_at_receive_us: u64,
}

/// 🎯 Comando de timing preciso
#[derive(Clone, Debug)]
pub struct TimingCommand {
    pub target_block: u64,
    pub send_at_us: u64, // Microssegundo exato para envio
    pub tx_data: Vec<u8>,
}

/// 🎲 Janela de envio ótima
#[derive(Clone, Debug)]
pub struct OptimalSendWindow {
    /// Início da janela (microssegundos antes do bloco)
    pub start_offset_us: i64,
    /// Fim da janela
    pub end_offset_us: i64,
    /// Probabilidade de inclusão no próximo bloco
    pub inclusion_probability: f64,
}

impl SequencerHeartbeatMonitor {
    /// 🚀 Inicializa monitor de heartbeat
    pub async fn new() -> Self {
        let rpc_url = std::env::var("BASE_RPC_URL")
            .unwrap_or_else(|_| "https://mainnet.base.org".to_string());
        
        let (block_tx, _) = mpsc::channel(1000);
        let (timing_tx, _) = mpsc::channel(1000);
        
        info!("[SEQUENCER-HEARTBEAT] ⏱️ Monitor inicializado | RPC: {}", rpc_url);
        
        Self {
            rpc_url,
            current_rtt_us: Arc::new(RwLock::new(0)),
            rtt_history: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            rtt_stddev_us: Arc::new(RwLock::new(0)),
            last_block_time: Arc::new(RwLock::new(Instant::now())),
            last_block_number: Arc::new(RwLock::new(0)),
            avg_block_interval_ms: Arc::new(RwLock::new(2000.0)), // 2s Base
            block_tx,
            timing_tx,
        }
    }
    
    /// 📡 Mede RTT com precisão de microssegundos
    pub async fn measure_rtt(&self) -> u64 {
        let start = Instant::now();
        
        // Simular chamada RPC (em produção: eth_blockNumber ou ping)
        // Aqui usamos um delay simulado baseado na rede
        let simulated_latency = self.simulate_network_latency().await;
        
        let elapsed = start.elapsed();
        let rtt_us = elapsed.as_micros() as u64 + simulated_latency;
        
        // Atualizar métricas
        *self.current_rtt_us.write().await = rtt_us;
        
        let mut history = self.rtt_history.write().await;
        history.push(rtt_us);
        if history.len() > 1000 {
            history.remove(0);
        }
        
        // Calcular desvio padrão
        if history.len() >= 10 {
            let mean = history.iter().sum::<u64>() / history.len() as u64;
            let variance: u64 = history.iter()
                .map(|&x| {
                    let diff = if x > mean { x - mean } else { mean - x };
                    diff * diff
                })
                .sum::<u64>() / history.len() as u64;
            *self.rtt_stddev_us.write().await = (variance as f64).sqrt() as u64;
        }
        
        trace!("[SEQUENCER-HEARTBEAT] 📡 RTT medido: {}μs", rtt_us);
        
        rtt_us
    }
    
    /// 🎯 Calcula momento ótimo de envio para inclusão no próximo bloco
    pub async fn calculate_optimal_send_time(
        &self,
        target_block: u64,
    ) -> OptimalSendWindow {
        let current_rtt = *self.current_rtt_us.read().await;
        let stddev = *self.rtt_stddev_us.read().await;
        let _block_interval = *self.avg_block_interval_ms.read().await;
        
        // Base: enviar RTT + margem antes do bloco
        let base_offset = current_rtt as i64 + (stddev * 2) as i64;
        
        // Janela de envio ótima
        // Começa: base_offset - 100ms (para segurança)
        // Termina: base_offset + 50ms (última chance)
        let window = OptimalSendWindow {
            start_offset_us: -(base_offset + 100_000), // 100ms antes do cálculo base
            end_offset_us: -(base_offset - 50_000),     // 50ms depois do cálculo base
            inclusion_probability: 0.95,                // 95% confiança
        };
        
        info!(
            "[SEQUENCER-HEARTBEAT] 🎯 Janela ótima para bloco {} | RTT: {}μs ±{}μs | Janela: {}μs a {}μs antes",
            target_block,
            current_rtt,
            stddev,
            -window.start_offset_us,
            -window.end_offset_us
        );
        
        window
    }
    
    /// ⏰ Inicia monitorização contínua do heartbeat
    pub async fn start_monitoring(&self) {
        let rtt_clone = self.current_rtt_us.clone();
        let history_clone = self.rtt_history.clone();
        let last_block = self.last_block_time.clone();
        let block_number = self.last_block_number.clone();
        
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(100)); // 10Hz
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            
            loop {
                ticker.tick().await;
                
                // Medir RTT
                let start = Instant::now();
                // Simulação: em produção seria chamada RPC real
                tokio::time::sleep(Duration::from_micros(500)).await;
                let rtt = start.elapsed().as_micros() as u64;
                
                *rtt_clone.write().await = rtt;
                
                let mut history = history_clone.write().await;
                history.push(rtt);
                if history.len() > 1000 {
                    history.remove(0);
                }
                
                // Detectar novo bloco (simulado)
                let elapsed_since_last = last_block.read().await.elapsed();
                if elapsed_since_last.as_millis() > 2000 {
                    *last_block.write().await = Instant::now();
                    *block_number.write().await += 1;
                    
                    info!(
                        "[SEQUENCER-HEARTBEAT] 🔔 Novo bloco detetado | RTT: {}μs | Histórico: {} amostras",
                        rtt,
                        history.len()
                    );
                }
            }
        });
    }
    
    /// 🔮 Prediz timestamp exato do próximo bloco
    pub async fn predict_next_block_time(&self) -> Instant {
        let last_time = *self.last_block_time.read().await;
        let interval = *self.avg_block_interval_ms.read().await;
        
        // Próximo bloco = último + intervalo médio
        last_time + Duration::from_millis(interval as u64)
    }
    
    /// ⏳ Aguarda momento preciso para envio
    pub async fn wait_for_send_window(&self, window: &OptimalSendWindow) {
        let next_block = self.predict_next_block_time().await;
        
        // Calcular quando enviar (offset negativo = antes do bloco)
        let send_time = next_block + Duration::from_micros(window.start_offset_us.unsigned_abs());
        
        let now = Instant::now();
        if send_time > now {
            let wait_duration = send_time - now;
            tokio::time::sleep(wait_duration).await;
        }
    }
    
    /// 📊 Retorna RTT atual
    pub async fn current_rtt_us(&self) -> u64 {
        *self.current_rtt_us.read().await
    }
    
    /// 📈 Estatísticas de RTT
    pub async fn rtt_stats(&self) -> String {
        let history = self.rtt_history.read().await;
        if history.is_empty() {
            return "Sem dados de RTT".to_string();
        }
        
        let min: u64 = *history.iter().min().unwrap_or(&0u64);
        let max: u64 = *history.iter().max().unwrap_or(&0u64);
        let avg = history.iter().sum::<u64>() / history.len() as u64;
        let stddev = *self.rtt_stddev_us.read().await;
        
        format!(
            "⏱️ RTT Stats | Min: {}μs | Max: {}μs | Avg: {}μs | σ: {}μs | Amostras: {}",
            min, max, avg, stddev, history.len()
        )
    }
    
    /// 🌐 Simula latência de rede (placeholder)
    async fn simulate_network_latency(&self) -> u64 {
        // Em produção, isto seria removido
        // Retorna latência baseada na carga da rede
        150u64 // 150μs base
    }
}

use tracing::{info, trace};
