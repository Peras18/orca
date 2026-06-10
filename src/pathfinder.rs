use alloy::primitives::{Address, U256};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tracing::{debug, trace};

use crate::types::{ArbitragePath, Hop, Pool, PriceUpdate, Fixed64};
use crate::contracts::DexType;

const MAX_ITERATIONS: usize = 1000;
const NEWTON_EPSILON: Fixed64 = 0.000001;
const GAS_COST_BASE: Fixed64 = 21000.0;
const GAS_COST_PER_HOP: Fixed64 = 100000.0;

pub struct Pathfinder {
    graph: Arc<RwLock<PriceGraph>>,
    pools: Arc<DashMap<Address, Pool>>,
    max_path_length: usize,
    token_index: Arc<DashMap<Address, usize>>,
}

struct PriceGraph {
    adjacency: HashMap<usize, Vec<Edge>>,
    tokens: Vec<Address>,
}

#[derive(Clone, Copy)]
struct Edge {
    target: usize,
    pool: Address,
    weight: Fixed64,
    fee: u32,
    #[allow(dead_code)]
    liquidity: Fixed64,
    dex_type: DexType,
}

impl Pathfinder {
    pub fn new(max_path_length: usize) -> Self {
        Self {
            graph: Arc::new(RwLock::new(PriceGraph {
                adjacency: HashMap::new(),
                tokens: Vec::new(),
            })),
            pools: Arc::new(DashMap::new()),
            max_path_length,
            token_index: Arc::new(DashMap::new()),
        }
    }

    pub fn update_price(&self, update: PriceUpdate) {
        let price = Self::sqrt_price_x96_to_f64(update.sqrt_price_x96);
        let liquidity = update.liquidity.to::<u64>() as Fixed64;
        
        trace!("Price update for pool {:?}: {}", update.pool, price);

        if let Some(pool_ref) = self.pools.get(&update.pool) {
            let pool = pool_ref.clone();
            drop(pool_ref);
            
            let token_in_idx = self.get_or_insert_token(pool.token_a);
            let token_out_idx = self.get_or_insert_token(pool.token_b);
            
            let fee_adjusted = price * ((1_000_000.0 - pool.fee as Fixed64) / 1_000_000.0);
            let log_price = -(fee_adjusted.ln() / 2.0_f64.ln());
            
            let mut graph = self.graph.write();
            
            graph.adjacency
                .entry(token_in_idx)
                .or_default()
                .push(Edge {
                    target: token_out_idx,
                    pool: pool.address,
                    weight: log_price,
                    fee: pool.fee,
                    liquidity,
                    dex_type: pool.dex_type,
                });
            
            let reverse_price = 1.0 / price;
            let reverse_fee_adjusted = reverse_price * ((1_000_000.0 - pool.fee as Fixed64) / 1_000_000.0);
            let reverse_log_price = -(reverse_fee_adjusted.ln() / 2.0_f64.ln());
            
            graph.adjacency
                .entry(token_out_idx)
                .or_default()
                .push(Edge {
                    target: token_in_idx,
                    pool: pool.address,
                    weight: reverse_log_price,
                    fee: pool.fee,
                    liquidity,
                    dex_type: pool.dex_type,
                });
        }
    }

    pub fn find_arbitrage(&self, start_token: Address) -> Option<ArbitragePath> {
        let graph = self.graph.read();
        let start_idx = *self.token_index.get(&start_token)?;
        
        let num_tokens = graph.tokens.len();
        if num_tokens == 0 {
            return None;
        }

        let mut distance = vec![Fixed64::MAX; num_tokens];
        let mut predecessor: Vec<Option<(usize, usize)>> = vec![None; num_tokens];
        let mut in_queue = vec![false; num_tokens];
        let mut queue = VecDeque::new();
        let mut iteration_count = vec![0usize; num_tokens];

        distance[start_idx] = 0.0;
        queue.push_back(start_idx);
        in_queue[start_idx] = true;

        while let Some(u) = queue.pop_front() {
            in_queue[u] = false;

            if let Some(edges) = graph.adjacency.get(&u) {
                for (edge_idx, edge) in edges.iter().enumerate() {
                    let v = edge.target;
                    let new_dist = distance[u] + edge.weight;

                    if new_dist < distance[v] {
                        distance[v] = new_dist;
                        predecessor[v] = Some((u, edge_idx));
                        iteration_count[v] = iteration_count[u] + 1;

                        if iteration_count[v] >= num_tokens {
                            return self.extract_cycle(&graph, &predecessor, v, start_token);
                        }

                        if !in_queue[v] {
                            queue.push_back(v);
                            in_queue[v] = true;
                        }
                    }
                }
            }
        }

        None
    }

    fn extract_cycle(
        &self,
        graph: &PriceGraph,
        predecessor: &[Option<(usize, usize)>],
        mut v: usize,
        start_token: Address,
    ) -> Option<ArbitragePath> {
        let mut path = vec![];
        let mut visited = std::collections::HashSet::new();
        
        while visited.insert(v) {
            if let Some((prev_node, edge_idx)) = predecessor[v] {
                if let Some(edges) = graph.adjacency.get(&prev_node) {
                    if let Some(edge) = edges.get(edge_idx) {
                        path.push((prev_node, v, *edge));
                    }
                }
                v = prev_node;
            } else {
                break;
            }
        }

        if path.len() < 2 || path.len() > self.max_path_length {
            return None;
        }

        path.reverse();

        let hops: Vec<Hop> = path
            .iter()
            .map(|(from, to, edge)| {
                let token_in = graph.tokens[*from];
                let token_out = graph.tokens[*to];
                Hop {
                    pool: edge.pool,
                    token_in,
                    token_out,
                    fee: edge.fee,
                    dex_type: edge.dex_type,
                }
            })
            .collect();

        let optimal_input = self.compute_optimal_input(&hops);
        let expected_profit = self.estimate_profit(&hops, optimal_input);

        Some(ArbitragePath {
            hops,
            input_token: start_token,
            optimal_input,
            expected_profit,
            profit_ratio: self.calculate_profit_ratio(expected_profit, optimal_input),
        })
    }

    fn compute_optimal_input(&self, hops: &[Hop]) -> U256 {
        let mut amount = 1.0;
        let mut prev_amount = 0.0;
        let mut iterations = 0;

        while f64::abs(amount - prev_amount) > NEWTON_EPSILON && iterations < MAX_ITERATIONS {
            prev_amount = amount;
            let profit_derivative = self.profit_derivative(hops, amount);
            let profit_second_derivative = self.profit_second_derivative(hops, amount);

            if profit_second_derivative.abs() < NEWTON_EPSILON {
                break;
            }

            amount = amount - profit_derivative / profit_second_derivative;
            
            if amount < 0.0 || amount == 0.0 {
                amount = 0.001;
            }

            iterations += 1;
        }

        let gas_cost = self.estimate_gas_cost(hops);
        let min_profitable = gas_cost * 1.1;
        
        if amount < min_profitable {
            amount = min_profitable;
        }

        U256::from(amount as u128)
    }

    fn profit_derivative(&self, hops: &[Hop], x: Fixed64) -> Fixed64 {
        let output = self.simulate_output(hops, x);
        let gas_cost = self.estimate_gas_cost(hops);
        
        let slippage_factor = self.aggregate_slippage(hops, x);
        let fee_factor: Fixed64 = hops.iter().fold(1.0, |acc, h| {
            acc * ((1_000_000.0 - h.fee as Fixed64) / 1_000_000.0)
        });

        output * fee_factor * slippage_factor - x - gas_cost
    }

    fn profit_second_derivative(&self, hops: &[Hop], x: Fixed64) -> Fixed64 {
        let delta = 0.001;
        let f_plus = self.profit_derivative(hops, x + delta);
        let f_minus = self.profit_derivative(hops, x - delta);
        
        (f_plus - f_minus) / (delta * 2.0)
    }

    fn simulate_output(&self, hops: &[Hop], input: Fixed64) -> Fixed64 {
        let mut amount = input;
        
        for hop in hops {
            if let Some(pool_ref) = self.pools.get(&hop.pool) {
                let pool = pool_ref.clone();
                drop(pool_ref);
                
                // Conversão segura: U256 (32 bytes) -> u64 (8 bytes) para Fixed64
                // Usa saturating para evitar overflow e manter proporção
                let reserve_in = if hop.token_in == pool.token_a {
                    let clamped: u64 = pool.reserve_a.to::<u64>().saturating_add(0);
                    Fixed64::from_be_bytes(clamped.to_be_bytes())
                } else {
                    let clamped: u64 = pool.reserve_b.to::<u64>().saturating_add(0);
                    Fixed64::from_be_bytes(clamped.to_be_bytes())
                };
                
                let reserve_out = if hop.token_in == pool.token_a {
                    let clamped: u64 = pool.reserve_b.to::<u64>().saturating_add(0);
                    Fixed64::from_be_bytes(clamped.to_be_bytes())
                } else {
                    let clamped: u64 = pool.reserve_a.to::<u64>().saturating_add(0);
                    Fixed64::from_be_bytes(clamped.to_be_bytes())
                };

                let amount_in_with_fee = amount * ((1_000_000.0 - hop.fee as Fixed64) / 1_000_000.0);
                let numerator = amount_in_with_fee * reserve_out;
                let denominator = reserve_in + amount_in_with_fee;
                
                if denominator == 0.0 {
                    return 0.0;
                }
                
                amount = numerator / denominator;
            }
        }
        
        amount
    }

    fn aggregate_slippage(&self, hops: &[Hop], input: Fixed64) -> Fixed64 {
        let mut total_slippage = 1.0;
        let mut amount = input;
        
        for hop in hops {
            if let Some(pool_ref) = self.pools.get(&hop.pool) {
                let pool = pool_ref.clone();
                drop(pool_ref);
                
                // Conversão segura: U256 -> u64 -> Fixed64
                let reserve = if hop.token_in == pool.token_a {
                    let clamped: u64 = pool.reserve_a.to::<u64>().saturating_add(0);
                    Fixed64::from_be_bytes(clamped.to_be_bytes())
                } else {
                    let clamped: u64 = pool.reserve_b.to::<u64>().saturating_add(0);
                    Fixed64::from_be_bytes(clamped.to_be_bytes())
                };
                
                let slippage = 1.0 - (amount / (amount + reserve));
                total_slippage = total_slippage * (1.0 - slippage);
                amount = amount * 0.997;
            }
        }
        
        total_slippage
    }

    fn estimate_gas_cost(&self, hops: &[Hop]) -> Fixed64 {
        let num_hops = hops.len() as Fixed64;
        GAS_COST_BASE + GAS_COST_PER_HOP * num_hops
    }

    fn estimate_profit(&self, hops: &[Hop], input: U256) -> U256 {
        let input_f64 = input.to::<u64>() as Fixed64;
        let output = self.simulate_output(hops, input_f64);
        let gas_cost_wei = self.estimate_gas_cost(hops) * 20_000_000_000.0;
        
        let profit = output - input_f64 - gas_cost_wei;
        
        if profit > 0.0 {
            U256::from(profit as u128)
        } else {
            U256::ZERO
        }
    }

    fn calculate_profit_ratio(&self, profit: U256, input: U256) -> Fixed64 {
        if input.is_zero() {
            return 0.0;
        }
        
        let profit_f = profit.to::<u64>() as Fixed64;
        let input_f = input.to::<u64>() as Fixed64;
        
        (profit_f / input_f) * 10000.0
    }

    fn get_or_insert_token(&self, token: Address) -> usize {
        *self.token_index.entry(token).or_insert_with(|| {
            let mut graph = self.graph.write();
            let idx = graph.tokens.len();
            graph.tokens.push(token);
            idx
        })
    }

    fn sqrt_price_x96_to_f64(sqrt_price_x96: U256) -> Fixed64 {
        let price_x96 = sqrt_price_x96 * sqrt_price_x96;
        let price = price_x96 / U256::from(2).pow(U256::from(192));
        
        (price.to::<u64>() as Fixed64) / 1e18
    }

    pub fn add_pool(&self, pool: Pool) {
        debug!("Adding pool: {:?}", pool.address);
        self.pools.insert(pool.address, pool);
    }
}
