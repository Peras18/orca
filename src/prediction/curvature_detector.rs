//! Ω-Curvature Detector — Geometria Diferencial de Reservas
//!
//! Deteta divergências de preço 2-3 blocos antes de serem visíveis
//! pela matemática AMM normal, usando o Laplaciano do log-ratio de
//! preços entre pools e covariância de reserves.
//!
//! Ω(t) = ∇²log(P_A/P_B) · σ(R_A, R_B) · e^(-λ·Δt)

use std::collections::{HashMap, VecDeque};
use alloy::primitives::Address;

const HISTORY_BLOCKS: usize = 8;
const DECAY_LAMBDA: f64 = 0.15;
pub const OMEGA_THRESHOLD: f64 = 0.002;

#[derive(Clone, Debug)]
struct ReserveSnapshot {
    block: u64,
    reserve_in: f64,
    reserve_out: f64,
}

impl ReserveSnapshot {
    fn price(&self) -> f64 {
        if self.reserve_in == 0.0 { return 0.0; }
        self.reserve_out / self.reserve_in
    }
}

#[derive(Default, Debug)]
struct PoolHistory {
    snapshots: VecDeque<ReserveSnapshot>,
}

impl PoolHistory {
    fn push(&mut self, block: u64, r_in: f64, r_out: f64) {
        if self.snapshots.len() >= HISTORY_BLOCKS {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(ReserveSnapshot { block, reserve_in: r_in, reserve_out: r_out });
    }

    fn prices(&self) -> Vec<f64> {
        self.snapshots.iter().map(|s| s.price()).collect()
    }

    fn last_block(&self) -> u64 {
        self.snapshots.back().map(|s| s.block).unwrap_or(0)
    }

    fn reserves_in(&self) -> Vec<f64> {
        self.snapshots.iter().map(|s| s.reserve_in).collect()
    }
}

#[derive(Debug, Clone)]
pub struct OmegaSignal {
    pub pool_a: Address,
    pub pool_b: Address,
    pub omega: f64,
    pub buy_a: bool,
    pub block: u64,
}

#[derive(Debug)]
pub struct CurvatureDetector {
    histories: HashMap<Address, PoolHistory>,
}

impl CurvatureDetector {
    pub fn new() -> Self {
        Self { histories: HashMap::new() }
    }

    pub fn update(&mut self, pool: Address, block: u64, reserve_in: f64, reserve_out: f64) {
        self.histories.entry(pool).or_default().push(block, reserve_in, reserve_out);
    }

    fn laplacian(series: &[f64]) -> f64 {
        let n = series.len();
        if n < 3 { return 0.0; }
        let t1 = series[n - 3];
        let t2 = series[n - 2];
        let t3 = series[n - 1];
        t3 - 2.0 * t2 + t1
    }

    fn covariance_normalized(a: &[f64], b: &[f64]) -> f64 {
        let n = a.len().min(b.len());
        if n < 2 { return 0.0; }
        let a = &a[a.len() - n..];
        let b = &b[b.len() - n..];
        let mean_a = a.iter().sum::<f64>() / n as f64;
        let mean_b = b.iter().sum::<f64>() / n as f64;
        let cov: f64 = a.iter().zip(b.iter())
            .map(|(x, y)| (x - mean_a) * (y - mean_b))
            .sum::<f64>() / n as f64;
        let std_a = (a.iter().map(|x| (x - mean_a).powi(2)).sum::<f64>() / n as f64).sqrt();
        let std_b = (b.iter().map(|x| (x - mean_b).powi(2)).sum::<f64>() / n as f64).sqrt();
        if std_a == 0.0 || std_b == 0.0 { return 0.0; }
        (cov / (std_a * std_b)).abs()
    }

    pub fn detect(&self, current_block: u64, pool_pairs: &[(Address, Address)]) -> Vec<OmegaSignal> {
        let mut signals = Vec::new();

        for &(addr_a, addr_b) in pool_pairs {
            let ha = match self.histories.get(&addr_a) {
                Some(h) if h.snapshots.len() >= 3 => h,
                _ => continue,
            };
            let hb = match self.histories.get(&addr_b) {
                Some(h) if h.snapshots.len() >= 3 => h,
                _ => continue,
            };

            let prices_a = ha.prices();
            let prices_b = hb.prices();
            let n = prices_a.len().min(prices_b.len());

            let log_ratio: Vec<f64> = prices_a[prices_a.len()-n..].iter()
                .zip(prices_b[prices_b.len()-n..].iter())
                .map(|(pa, pb)| if *pb > 0.0 { (pa / pb).ln() } else { 0.0 })
                .collect();

            let laplacian = Self::laplacian(&log_ratio);
            let reserves_a = ha.reserves_in();
            let reserves_b = hb.reserves_in();
            let sigma = Self::covariance_normalized(&reserves_a, &reserves_b);

            let delta_t = current_block.saturating_sub(ha.last_block().max(hb.last_block())) as f64;
            let decay = (-DECAY_LAMBDA * delta_t).exp();

            let omega = laplacian.abs() * sigma * decay;

            if omega >= OMEGA_THRESHOLD {
                let buy_a = laplacian < 0.0;
                signals.push(OmegaSignal {
                    pool_a: addr_a,
                    pool_b: addr_b,
                    omega,
                    buy_a,
                    block: current_block,
                });
            }
        }

        signals.sort_by(|a, b| b.omega.partial_cmp(&a.omega).unwrap_or(std::cmp::Ordering::Equal));
        signals
    }
}