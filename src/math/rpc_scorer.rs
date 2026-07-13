//! 📡 Scorer adaptativo de RPCs -- média móvel exponencial de latência e
//! taxa de sucesso por endpoint. Em vez de correr sempre contra TODOS os
//! RPCs em paralelo (auto-infligindo contenção/rate-limit em endpoints
//! partilhados gratuitos -- hipótese confirmada pela diferença entre teste
//! isolado ~250ms e produção ~1267ms), seleciona dinamicamente apenas os
//! N melhores por desempenho histórico real.

use dashmap::DashMap;

#[derive(Debug, Clone, Copy)]
struct RpcStats {
    ema_latency_ms: f64,
    success_rate: f64, // EMA de 0.0-1.0
}

#[derive(Debug)]
pub struct RpcScorer {
    stats: DashMap<String, RpcStats>,
    alpha: f64, // fator de suavização EMA
}

impl RpcScorer {
    pub fn new() -> Self {
        Self { stats: DashMap::new(), alpha: 0.2 }
    }

    pub fn record(&self, rpc_url: &str, latency_ms: f64, success: bool) {
        let mut entry = self.stats.entry(rpc_url.to_string()).or_insert(RpcStats {
            ema_latency_ms: latency_ms,
            success_rate: if success { 1.0 } else { 0.0 },
        });
        entry.ema_latency_ms = self.alpha * latency_ms + (1.0 - self.alpha) * entry.ema_latency_ms;
        let s = if success { 1.0 } else { 0.0 };
        entry.success_rate = self.alpha * s + (1.0 - self.alpha) * entry.success_rate;
    }

    /// Seleciona os `n` melhores RPCs por score (latência baixa + sucesso alto).
    /// RPCs sem histórico (novos) ficam sempre incluídos -- otimismo inicial,
    /// para não excluir permanentemente algo nunca testado.
    pub fn top_n<'a>(&self, candidates: &'a [String], n: usize) -> Vec<String> {
        let mut scored: Vec<(String, f64)> = candidates.iter().map(|url| {
            match self.stats.get(url) {
                Some(s) if s.success_rate > 0.0 => {
                    // score menor = melhor (latência penalizada por falhas)
                    let score = s.ema_latency_ms / s.success_rate.max(0.05);
                    (url.clone(), score)
                }
                _ => (url.clone(), 0.0), // sem histórico -- prioridade máxima (explorar)
            }
        }).collect();
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(n.max(1)).map(|(url, _)| url).collect()
    }
}
