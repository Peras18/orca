//! 🔬 Spectral Graph Theory — Expansores de Ramanujan para detetar arb
//! Usa o segundo valor próprio da matriz de adjacência normalizada
//! para encontrar "pontes" de liquidez entre clusters de pools
//! 
//! Expansor de Ramanujan: grafo onde λ₂ ≤ 2√(d-1)
//! Pools com alto betweenness centrality = pontes de arbitragem

use alloy::primitives::Address;
use std::collections::HashMap;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PoolNode {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub tvl_eth: f64,
    pub volume_24h: f64,
}

#[derive(Debug)]
pub struct SpectralArbDetector {
    /// Nós do grafo (pools)
    nodes: Vec<PoolNode>,
    /// Índice por endereço
    node_index: HashMap<Address, usize>,
    /// Matriz de adjacência ponderada por TVL
    adjacency: Vec<Vec<f64>>,
    /// Centralidade de betweenness (importância de cada pool)
    pub betweenness: Vec<f64>,
    /// Clusters de pools (pools no mesmo cluster têm preços correlacionados)
    pub clusters: Vec<Vec<usize>>,
    /// Pools "ponte" entre clusters (maior potencial de arb)
    pub bridge_pools: Vec<usize>,
}

impl SpectralArbDetector {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            node_index: HashMap::new(),
            adjacency: Vec::new(),
            betweenness: Vec::new(),
            clusters: Vec::new(),
            bridge_pools: Vec::new(),
        }
    }

    pub fn add_pool(&mut self, pool: PoolNode) {
        let idx = self.nodes.len();
        self.node_index.insert(pool.address, idx);
        self.nodes.push(pool);
        // Expandir matriz de adjacência
        for row in &mut self.adjacency {
            row.push(0.0);
        }
        self.adjacency.push(vec![0.0; self.nodes.len()]);
        self.betweenness.push(0.0);
    }

    /// Adiciona edge entre dois pools (partilham um token)
    pub fn add_edge(&mut self, pool_a: Address, pool_b: Address, weight: f64) {
        if let (Some(&i), Some(&j)) = (self.node_index.get(&pool_a), self.node_index.get(&pool_b)) {
            self.adjacency[i][j] = weight;
            self.adjacency[j][i] = weight;
        }
    }

    /// Calcula a matriz Laplaciana normalizada: L = I - D^{-1/2} A D^{-1/2}
    fn normalized_laplacian(&self) -> Vec<Vec<f64>> {
        let n = self.nodes.len();
        if n == 0 { return Vec::new(); }

        // Graus ponderados
        let degrees: Vec<f64> = (0..n)
            .map(|i| self.adjacency[i].iter().sum::<f64>())
            .collect();

        let mut laplacian = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    laplacian[i][j] = 1.0;
                } else if degrees[i] > 0.0 && degrees[j] > 0.0 {
                    laplacian[i][j] = -self.adjacency[i][j]
                        / (degrees[i] * degrees[j]).sqrt();
                }
            }
        }
        laplacian
    }

    /// Power iteration para estimar o segundo valor próprio (λ₂)
    /// λ₂ baixo = grafo bem conectado (poucas pontes)
    /// λ₂ alto = grafo fragmentado (muitas pontes = oportunidades de arb)
    pub fn estimate_lambda2(&self) -> f64 {
        let n = self.nodes.len();
        if n < 2 { return 0.0; }

        let lap = self.normalized_laplacian();

        // Iniciar com vetor aleatório ortogonal a [1,1,...,1]
        let mut v: Vec<f64> = (0..n).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        let norm: f64 = v.iter().map(|x| x*x).sum::<f64>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm.max(1e-10));

        let mut lambda = 0.0;
        for _ in 0..50 { // 50 iterações
            // v_new = L * v
            let mut v_new = vec![0.0; n];
            for i in 0..n {
                for j in 0..n {
                    v_new[i] += lap[i][j] * v[j];
                }
            }
            // Rayleigh quotient: λ = v^T L v / v^T v
            let num: f64 = v.iter().zip(&v_new).map(|(a,b)| a*b).sum();
            let den: f64 = v.iter().map(|x| x*x).sum::<f64>();
            lambda = if den > 1e-10 { num / den } else { 0.0 };

            // Normalizar
            let norm: f64 = v_new.iter().map(|x| x*x).sum::<f64>().sqrt();
            if norm < 1e-10 { break; }
            v = v_new.iter().map(|x| x / norm).collect();
        }
        lambda
    }

    /// Betweenness centrality aproximada via BFS
    /// Pools com alto betweenness são pontes críticas de arb
    pub fn compute_betweenness(&mut self) {
        let n = self.nodes.len();
        if n == 0 { return; }
        self.betweenness = vec![0.0; n];

        for s in 0..n {
            // BFS a partir de s
            let mut dist = vec![-1i32; n];
            let mut sigma = vec![0u64; n]; // caminhos mínimos passando por cada nó
            dist[s] = 0;
            sigma[s] = 1;
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(s);
            let mut stack = Vec::new();
            let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];

            while let Some(v) = queue.pop_front() {
                stack.push(v);
                for w in 0..n {
                    if self.adjacency[v][w] > 0.0 {
                        if dist[w] < 0 {
                            queue.push_back(w);
                            dist[w] = dist[v] + 1;
                        }
                        if dist[w] == dist[v] + 1 {
                            sigma[w] += sigma[v];
                            pred[w].push(v);
                        }
                    }
                }
            }

            // Accumulate dependencies
            let mut delta = vec![0.0f64; n];
            while let Some(w) = stack.pop() {
                for &v in &pred[w] {
                    if sigma[w] > 0 {
                        delta[v] += (sigma[v] as f64 / sigma[w] as f64) * (1.0 + delta[w]);
                    }
                }
                if w != s {
                    self.betweenness[w] += delta[w];
                }
            }
        }

        // Identificar bridge pools (top 20% por betweenness)
        let mut sorted_bt: Vec<(usize, f64)> = self.betweenness.iter()
            .enumerate()
            .map(|(i, &b)| (i, b))
            .collect();
        sorted_bt.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let top_n = (n / 5).max(1);
        self.bridge_pools = sorted_bt.iter().take(top_n).map(|(i, _)| *i).collect();

        info!(
            "[SPECTRAL] {} pools | λ₂≈{:.4} | {} bridge pools identificadas",
            n,
            self.estimate_lambda2(),
            self.bridge_pools.len()
        );
    }

    /// Retorna as bridge pools ordenadas por potencial de arb
    pub fn top_bridge_pools(&self) -> Vec<(Address, f64)> {
        self.bridge_pools.iter()
            .filter_map(|&i| {
                self.nodes.get(i).map(|node| (node.address, self.betweenness[i]))
            })
            .collect()
    }

    pub fn pool_count(&self) -> usize {
        self.nodes.len()
    }
}
