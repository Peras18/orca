use dashmap::DashMap;

#[derive(Clone, Debug, Default)]
pub struct PoolScorer {
    // pool_address -> score (0 a 1000, inteiro)
    scores: DashMap<String, u32>,
    // pool_address -> número de swaps recebidos
    swap_counts: DashMap<String, u64>,
}

impl PoolScorer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_swap_received(&self, pool: &str) {
        let mut entry = self.swap_counts.entry(pool.to_string()).or_default();
        *entry += 1;
        let count = *entry;

        // Score base: frequência de swaps (max 500)
        let freq_score = (count.min(100) * 5) as u32;
        self.scores.insert(pool.to_string(), freq_score);
    }

    pub fn on_opportunity_found(&self, pool: &str, profit_wei: u128) {
        // Bonus por oportunidade detectada (max 500)
        let bonus = (profit_wei / 1_000_000_000_000_000u128).min(500) as u32;
        let mut score = self.scores.entry(pool.to_string()).or_default();
        *score = score.saturating_add(bonus).min(1000);
    }

    pub fn get_score(&self, pool: &str) -> u32 {
        self.scores.get(pool).map(|v| *v).unwrap_or(0)
    }

    pub fn top_pools(&self, n: usize) -> Vec<String> {
        let mut all: Vec<(String, u32)> = self
            .scores
            .iter()
            .map(|e| (e.key().clone(), *e.value()))
            .collect();
        all.sort_by(|a, b| b.1.cmp(&a.1));
        all.into_iter().take(n).map(|(p, _)| p).collect()
    }
}

