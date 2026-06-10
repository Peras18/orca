//! 📡 Entropia de Transferência — detetar causalidade entre pools
//! Se pool A mexe → pool B vai mexer em ~200ms
//! Permite antecipar spreads ANTES de aparecerem

#[derive(Debug, Clone)]
pub struct TransferEntropyDetector {
    /// Histórico de preços por pool (últimos N blocos)
    price_history: std::collections::HashMap<alloy::primitives::Address, Vec<f64>>,
    /// Matriz de causalidade: (pool_a, pool_b) → entropia (0-1)
    causality_matrix: std::collections::HashMap<
        (alloy::primitives::Address, alloy::primitives::Address), f64
    >,
    /// Janela temporal para análise
    window: usize,
    /// Threshold para considerar causalidade significativa
    threshold: f64,
}

impl TransferEntropyDetector {
    pub fn new(window: usize) -> Self {
        Self {
            price_history: std::collections::HashMap::new(),
            causality_matrix: std::collections::HashMap::new(),
            window,
            threshold: 0.1, // 10% de entropia mínima
        }
    }

    /// Regista novo preço para uma pool
    pub fn record_price(&mut self, pool: alloy::primitives::Address, price: f64) {
        let history = self.price_history.entry(pool).or_insert_with(Vec::new);
        history.push(price);
        if history.len() > self.window * 2 {
            history.remove(0);
        }
    }

    /// Calcula entropia de transferência de pool_a → pool_b
    /// TE(A→B) = H(B_future | B_past) - H(B_future | B_past, A_past)
    /// Valor alto = A causa B (A move primeiro, B segue)
    pub fn compute_te(&self,
        pool_a: alloy::primitives::Address,
        pool_b: alloy::primitives::Address,
    ) -> f64 {
        let hist_a = match self.price_history.get(&pool_a) {
            Some(h) if h.len() >= self.window => h,
            _ => return 0.0,
        };
        let hist_b = match self.price_history.get(&pool_b) {
            Some(h) if h.len() >= self.window => h,
            _ => return 0.0,
        };

        let n = self.window.min(hist_a.len()).min(hist_b.len());
        if n < 4 { return 0.0; }

        // Discretizar em 3 bins: down, flat, up
        let discretize = |vals: &[f64]| -> Vec<i8> {
            vals.windows(2).map(|w| {
                let delta = (w[1] - w[0]) / w[0].abs().max(1e-10);
                if delta > 0.001 { 1 }
                else if delta < -0.001 { -1 }
                else { 0 }
            }).collect()
        };

        let da: Vec<i8> = discretize(&hist_a[hist_a.len()-n..]);
        let db: Vec<i8> = discretize(&hist_b[hist_b.len()-n..]);

        if da.len() < 3 || db.len() < 3 { return 0.0; }

        // H(B_t | B_{t-1}) — entropia condicional de B dado B passado
        let h_b_given_bpast = self.conditional_entropy(&db, &db);

        // H(B_t | B_{t-1}, A_{t-1}) — entropia condicional de B dado B e A passados
        let h_b_given_both = self.conditional_entropy_joint(&db, &da);

        // TE = redução de incerteza quando adicionamos A
        let te = (h_b_given_bpast - h_b_given_both).max(0.0);
        te
    }

    fn conditional_entropy(&self, target: &[i8], source: &[i8]) -> f64 {
        let n = (target.len() - 1).min(source.len() - 1);
        if n == 0 { return 0.0; }

        let mut joint: std::collections::HashMap<(i8, i8), u32> = std::collections::HashMap::new();
        let mut marginal: std::collections::HashMap<i8, u32> = std::collections::HashMap::new();

        for i in 0..n {
            *joint.entry((source[i], target[i+1])).or_insert(0) += 1;
            *marginal.entry(source[i]).or_insert(0) += 1;
        }

        let n_f = n as f64;
        let mut entropy = 0.0;
        for ((s, _t), &count) in &joint {
            let p_joint = count as f64 / n_f;
            let p_source = *marginal.get(s).unwrap_or(&1) as f64 / n_f;
            if p_joint > 0.0 && p_source > 0.0 {
                entropy -= p_joint * (p_joint / p_source).ln();
            }
        }
        entropy
    }

    fn conditional_entropy_joint(&self, target: &[i8], source_a: &[i8]) -> f64 {
        let n = (target.len() - 1).min(source_a.len() - 1);
        if n == 0 { return 0.0; }

        let mut joint3: std::collections::HashMap<(i8, i8, i8), u32> = std::collections::HashMap::new();
        let mut joint2: std::collections::HashMap<(i8, i8), u32> = std::collections::HashMap::new();

        for i in 0..n {
            let key3 = (target[i], source_a[i], target[i+1]);
            let key2 = (target[i], source_a[i]);
            *joint3.entry(key3).or_insert(0) += 1;
            *joint2.entry(key2).or_insert(0) += 1;
        }

        let n_f = n as f64;
        let mut entropy = 0.0;
        for ((b_past, a_past, _b_fut), &count) in &joint3 {
            let p3 = count as f64 / n_f;
            let p2 = *joint2.get(&(*b_past, *a_past)).unwrap_or(&1) as f64 / n_f;
            if p3 > 0.0 && p2 > 0.0 {
                entropy -= p3 * (p3 / p2).ln();
            }
        }
        entropy
    }

    /// Atualiza matriz de causalidade para todas as pools conhecidas
    pub fn update_causality(&mut self) {
        let pools: Vec<alloy::primitives::Address> = self.price_history.keys().cloned().collect();
        for i in 0..pools.len() {
            for j in 0..pools.len() {
                if i == j { continue; }
                let te = self.compute_te(pools[i], pools[j]);
                if te > self.threshold {
                    self.causality_matrix.insert((pools[i], pools[j]), te);
                }
            }
        }
    }

    /// Retorna pools que são causadas por pool_a (ordenadas por força causal)
    pub fn get_caused_pools(
        &self,
        pool_a: alloy::primitives::Address,
    ) -> Vec<(alloy::primitives::Address, f64)> {
        let mut caused: Vec<_> = self.causality_matrix.iter()
            .filter(|((a, _), _)| *a == pool_a)
            .map(|((_, b), &te)| (*b, te))
            .collect();
        caused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        caused
    }

    pub fn pool_count(&self) -> usize {
        self.price_history.len()
    }
}
