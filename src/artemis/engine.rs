use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use super::{MevEvent, Strategy, StrategyContext};
use crate::artemis::collector_v2::LogCollectorV2;

/// Engine principal do Artemis
pub struct ArtemisEngine<S: Strategy> {
    collector: Arc<LogCollectorV2>,
    strategy: Arc<RwLock<S>>,
    context: Arc<StrategyContext>,
}

impl<S: Strategy + Send + Sync + 'static> ArtemisEngine<S> {
    pub fn new(
        collector: Arc<LogCollectorV2>,
        strategy: S,
        context: StrategyContext,
    ) -> Self {
        Self {
            collector,
            strategy: Arc::new(RwLock::new(strategy)),
            context: Arc::new(context),
        }
    }

    /// Inicia o motor de processamento de eventos
    pub async fn run(self: Arc<Self>) -> eyre::Result<()> {
        info!("ArtemisEngine: Iniciando processamento de eventos");

        // Spawn worker threads para processamento paralelo
        let num_workers = std::cmp::max(1, num_cpus::get() - 1);
        info!("ArtemisEngine: {} workers de processamento", num_workers);

        let mut handles = Vec::with_capacity(num_workers);

        for worker_id in 0..num_workers {
            let rx = self.collector.subscribe_events();
            let strategy = self.strategy.clone();
            let context = self.context.clone();
            
            let handle = tokio::spawn(async move {
                Self::event_worker(worker_id, rx, strategy, context).await;
            });
            
            handles.push(handle);
        }

        // Aguardar workers (nunca deve terminar em operação normal)
        for handle in handles {
            if let Err(e) = handle.await {
                error!("Worker falhou: {}", e);
            }
        }

        Ok(())
    }

    /// Worker thread que processa eventos
    async fn event_worker(
        worker_id: usize,
        mut rx: tokio::sync::broadcast::Receiver<MevEvent>,
        strategy: Arc<RwLock<S>>,
        context: Arc<StrategyContext>,
    ) {
        info!("Worker {}: Iniciado", worker_id);
        
        let mut processed = 0u64;
        let mut last_report = std::time::Instant::now();

        loop {
            match rx.recv().await {
                Ok(event) => {
                    processed += 1;
                    
                    // Processar evento sem bloquear em caso de erro
                    let result = async {
                        let mut strategy_guard = strategy.write().await;
                        strategy_guard.process_event(event, &context).await
                    }.await;

                    if let Err(e) = result {
                        trace!("Worker {}: Erro ao processar evento: {}", worker_id, e);
                    }

                    // Reportar estatísticas a cada 10 segundos
                    if last_report.elapsed().as_secs() >= 10 {
                        debug!("Worker {}: {} eventos/s", worker_id, processed / 10);
                        processed = 0;
                        last_report = std::time::Instant::now();
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    warn!("Worker {}: Canal fechado - terminando", worker_id);
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Worker {}: Lagged {} eventos - saltando", worker_id, n);
                    continue;
                }
            }
        }
    }
}

/// Configuração do contexto de estratégia
#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub num_workers: usize,
    pub batch_size: usize,
    pub max_latency_ms: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            num_workers: num_cpus::get(),
            batch_size: 100,
            max_latency_ms: 10,
        }
    }
}
