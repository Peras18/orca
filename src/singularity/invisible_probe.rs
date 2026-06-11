//! INVISIBLE PROBING
//! Envia transações de teste com gas_price mínimo para mapear 
//! quais nós da Alchemy são os mais rápidos a propagar para o sequenciador.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use std::time::{Duration, Instant};

/// 👁️ Sonda Invisível
#[derive(Clone, Debug)]
pub struct InvisibleProber {
    /// Nós a testar
    target_nodes: Arc<RwLock<Vec<String>>>,
    /// Resultados de latência por nó
    latency_map: Arc<RwLock<HashMap<String, NodeMetrics>>>,
    /// Histórico de probes
    probe_history: Arc<RwLock<Vec<ProbeResult>>>,
    /// Melhor nó atual
    best_node: Arc<RwLock<String>>,
    /// Contador de probes
    probe_count: Arc<RwLock<u64>>,
    /// Canal de comandos
    cmd_tx: mpsc::Sender<ProbeCommand>,
}

/// 🖥️ Métricas de um nó
#[derive(Clone, Debug)]
pub struct NodeMetrics {
    /// URL do nó
    pub url: String,
    /// Latência média (ms)
    pub avg_latency_ms: f64,
    /// Latência mínima (ms)
    pub min_latency_ms: f64,
    /// Latência máxima (ms)
    pub max_latency_ms: f64,
    /// Desvio padrão
    pub stddev_ms: f64,
    /// Taxa de sucesso (0.0-1.0)
    pub success_rate: f64,
    /// Score composto (menor = melhor)
    pub composite_score: f64,
    /// Última probe
    pub last_probed_at: Instant,
    /// Rank atual
    pub rank: usize,
}

/// 🎯 Resultado de probe
#[derive(Clone, Debug)]
pub struct ProbeResult {
    /// Nó testado
    pub node: String,
    /// Timestamp do envio
    pub sent_at: Instant,
    /// Timestamp da confirmação
    pub confirmed_at: Option<Instant>,
    /// Latência medida (se confirmado)
    pub latency_ms: Option<f64>,
    /// Gas price usado (mínimo)
    pub gas_price_wei: u64,
    /// Sucesso da probe
    pub success: bool,
    /// Tipo de probe
    pub probe_type: ProbeType,
}

/// 🎲 Tipo de probe
#[derive(Clone, Debug, PartialEq)]
pub enum ProbeType {
    /// Ping simples (eth_blockNumber)
    Ping,
    /// Transação dummy (com gas mínimo)
    DummyTx,
    /// Teste de propagação
    PropagationTest,
    /// Latência para sequenciador
    SequencerLatency,
}

/// 📡 Comando de probe
#[derive(Clone, Debug)]
pub struct ProbeCommand {
    pub node: String,
    pub probe_type: ProbeType,
    pub gas_price: u64,
}

/// 🏆 Ranking de nós
#[derive(Clone, Debug)]
pub struct NodeRanking {
    pub url: String,
    pub score: f64,
    pub rank: usize,
    pub latency_ms: f64,
}

impl InvisibleProber {
    /// 🚀 Inicializa sonda invisível
    pub async fn new() -> Self {
        let (cmd_tx, _) = mpsc::channel(1000);
        
        // Nós Alchemy (exemplo - em produção carregar de config)
        // Carregar RPCs do env + fallbacks públicos gratuitos
        let mut nodes: Vec<String> = std::env::var("RPC_HTTP_URLS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect();
        // Fallbacks públicos gratuitos sempre disponíveis
        for fallback in &[
            "https://base.llamarpc.com",
            "https://base.meowrpc.com",
            "https://1rpc.io/base",
            "https://base.gateway.tenderly.co/3keLiPzUyOTAczrG9yoUfh",
        ] {
            if !nodes.contains(&fallback.to_string()) {
                nodes.push(fallback.to_string());
            }
        }
        
        let mut latency_map = HashMap::new();
        for node in &nodes {
            latency_map.insert(node.clone(), NodeMetrics {
                url: node.clone(),
                avg_latency_ms: 1000.0, // Inicial: 1s
                min_latency_ms: 1000.0,
                max_latency_ms: 1000.0,
                stddev_ms: 0.0,
                success_rate: 0.0,
                composite_score: 1000.0,
                last_probed_at: Instant::now(),
                rank: nodes.len(), // Último inicialmente
            });
        }
        
        info!("[INVISIBLE-PROBE] 👁️ Sonda inicializada | {} nós para mapear", nodes.len());
        info!("[INVISIBLE-PROBE] 🎯 Objectivo: Identificar nó ótimo para o sequenciador Coinbase");
        
        Self {
            target_nodes: Arc::new(RwLock::new(nodes)),
            latency_map: Arc::new(RwLock::new(latency_map)),
            probe_history: Arc::new(RwLock::new(Vec::new())),
            best_node: Arc::new(RwLock::new("unknown".to_string())),
            probe_count: Arc::new(RwLock::new(0)),
            cmd_tx,
        }
    }
    
    /// 📡 Executa probe invisível
    pub async fn probe_node(&self, node_url: &str, probe_type: ProbeType) -> ProbeResult {
        let start = Instant::now();
        let gas_price = 1_000_000_000u64; // 1 gwei mínimo ("invisível")
        
        // Simulação de chamada RPC
        // Em produção: chamar eth_blockNumber ou enviar dummy tx
        let simulated_latency = match probe_type {
            ProbeType::Ping => 50.0,      // 50ms
            ProbeType::DummyTx => 150.0,  // 150ms
            ProbeType::PropagationTest => 300.0, // 300ms
            ProbeType::SequencerLatency => 200.0, // 200ms
        };
        
        tokio::time::sleep(Duration::from_millis(simulated_latency as u64)).await;
        
        let latency = start.elapsed().as_millis() as f64 + simulated_latency;
        let success = latency < 500.0; // <500ms = sucesso
        
        let probe_type_copy = probe_type.clone();
        
        let result = ProbeResult {
            node: node_url.to_string(),
            sent_at: start,
            confirmed_at: Some(Instant::now()),
            latency_ms: Some(latency),
            gas_price_wei: gas_price,
            success,
            probe_type: probe_type_copy,
        };
        
        *self.probe_count.write().await += 1;
        
        // Guardar resultado
        self.probe_history.write().await.push(result.clone());
        
        // Atualizar métricas do nó
        self.update_node_metrics(node_url, &result).await;
        
        trace!(
            "[INVISIBLE-PROBE] 📡 {} | Type: {:?} | Latência: {:.1}ms | Gas: {} wei",
            node_url,
            probe_type,
            latency,
            gas_price
        );
        
        result
    }
    
    /// 📊 Atualiza métricas de nó com novo resultado
    async fn update_node_metrics(&self, node_url: &str, result: &ProbeResult) {
        let mut map = self.latency_map.write().await;
        
        if let Some(metrics) = map.get_mut(node_url) {
            if let Some(latency) = result.latency_ms {
                // Atualizar média móvel
                let alpha = 0.3; // Fator de smoothing
                metrics.avg_latency_ms = metrics.avg_latency_ms * (1.0 - alpha) + latency * alpha;
                
                // Atualizar min/max
                if latency < metrics.min_latency_ms {
                    metrics.min_latency_ms = latency;
                }
                if latency > metrics.max_latency_ms {
                    metrics.max_latency_ms = latency;
                }
                
                // Calcular desvio padrão
                let diff = latency - metrics.avg_latency_ms;
                metrics.stddev_ms = f64::max(metrics.stddev_ms * 0.9 + diff.abs() * 0.1, 0.0);
                
                // Taxa de sucesso
                let history = self.probe_history.read().await;
                let node_results: Vec<_> = history.iter()
                    .filter(|r| r.node == node_url)
                    .collect();
                
                if !node_results.is_empty() {
                    let successes = node_results.iter().filter(|r| r.success).count();
                    metrics.success_rate = successes as f64 / node_results.len() as f64;
                }
                
                // Score composto (menor = melhor)
                // Pondera latência média, estabilidade e sucesso
                metrics.composite_score = 
                    metrics.avg_latency_ms * 0.5 +           // 50% latência
                    metrics.stddev_ms * 2.0 +                 // 20% estabilidade
                    (1.0 - metrics.success_rate) * 300.0;    // 30% confiabilidade
                
                metrics.last_probed_at = Instant::now();
            }
        }
        
        // Recalcular rankings
        self.recalculate_rankings().await;
    }
    
    /// 🏆 Recalcula rankings de todos os nós
    async fn recalculate_rankings(&self) {
        let mut map = self.latency_map.write().await;
        
        // Converter para vetor para ordenar
        let mut nodes: Vec<_> = map.values_mut().collect();
        
        // Ordenar por score composto
        nodes.sort_by(|a, b| {
            a.composite_score.partial_cmp(&b.composite_score).unwrap()
        });
        
        // Atualizar ranks
        for (rank, node) in nodes.iter_mut().enumerate() {
            node.rank = rank + 1;
        }
        
        // Atualizar melhor nó
        if let Some(best) = nodes.first() {
            *self.best_node.write().await = best.url.clone();
        }
    }
    
    /// 🔬 Inicia probing contínuo
    pub async fn start_continuous_probing(&self) {
        let nodes = self.target_nodes.clone();
        let prober = self.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            
            loop {
                interval.tick().await;
                
                let node_list = nodes.read().await.clone();
                
                for node in &node_list {
                    // Probe ping (mais frequente)
                    prober.probe_node(node, ProbeType::Ping).await;
                    
                    // A cada 5 probes, fazer probe de latência para sequenciador
                    let count = *prober.probe_count.read().await;
                    if count % 5 == 0 {
                        prober.probe_node(node, ProbeType::SequencerLatency).await;
                    }
                }
                
                // Log de status
                let best = prober.best_node().await;
                let count = *prober.probe_count.read().await;
                
                info!(
                    "[INVISIBLE-PROBE] 📊 Probing contínuo | Total: {} probes | Melhor nó: {}",
                    count,
                    best
                );
            }
        });
    }
    
    /// 🎯 Retorna melhor nó identificado
    pub async fn best_node(&self) -> String {
        self.best_node.read().await.clone()
    }
    
    /// 📊 Retorna ranking de nós
    pub async fn node_rankings(&self) -> Vec<NodeRanking> {
        let map = self.latency_map.read().await;
        
        let mut rankings: Vec<_> = map.values()
            .map(|m| NodeRanking {
                url: m.url.clone(),
                score: m.composite_score,
                rank: m.rank,
                latency_ms: m.avg_latency_ms,
            })
            .collect();
        
        rankings.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        rankings
    }
    
    /// 🔢 Retorna número de nós mapeados
    pub async fn mapped_nodes(&self) -> usize {
        self.latency_map.read().await.len()
    }
    
    /// 📈 Estatísticas completas
    pub async fn stats(&self) -> String {
        let count = *self.probe_count.read().await;
        let best = self.best_node().await;
        let mapped = self.mapped_nodes().await;
        
        let map = self.latency_map.read().await;
        let avg_latency = map.values()
            .map(|m| m.avg_latency_ms)
            .sum::<f64>() / mapped as f64;
        
        format!(
            "👁️ Invisible Prober | Nós: {} | Probes: {} | Melhor: {} | Latência média: {:.1}ms",
            mapped, count, best, avg_latency
        )
    }
}

use tracing::{info, trace};
