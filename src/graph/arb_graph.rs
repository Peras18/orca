//! 🕸️ Arbitrage Graph - Multi-hop Cycle Detection
//!
//! Implementa grafo direcionado de tokens com:
//! - Ciclos 2-hop (A -> B -> A)
//! - Ciclos 3-hop triangulares (A -> B -> C -> A)
//! - Deteção de duplicados
//! - Ordenação por lucro líquido
//! - Suporte para pools mistos V2+Stable+V3

use alloy::primitives::{address, Address, U256};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info, trace, warn};

use crate::cache::pool_cache::{PoolCache, PoolState};
use crate::contracts::DexType;
use crate::types::Hop;

/// Tamanho máximo de path para SmallVec (evita alocação heap)
const MAX_PATH_HOPS: usize = 4;

/// Representação de uma aresta do grafo (pool)
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub fee: u32,
    pub dex_type: DexType,
    /// Reserve de entrada
    pub reserve_in: U256,
    /// Reserve de saída
    pub reserve_out: U256,
    /// Decimals do token de entrada
    pub decimals_in: u8,
    /// Decimals do token de saída
    pub decimals_out: u8,
    /// sqrt(price) * 2^96 — apenas V3/Slipstream
    pub sqrt_price_x96: Option<u128>,
    /// Liquidez ativa no tick — apenas V3/Slipstream
    pub liquidity: Option<u128>,
}

impl Edge {
    /// Cria edge a partir de PoolState
    pub fn from_pool(pool: &PoolState, token_in: Address) -> Option<Self> {
        let (token_out, reserve_in, reserve_out, decimals_in, decimals_out) =
            if pool.token0 == token_in {
                (
                    pool.token1,
                    pool.reserve0,
                    pool.reserve1,
                    pool.decimals0,
                    pool.decimals1,
                )
            } else if pool.token1 == token_in {
                (
                    pool.token0,
                    pool.reserve1,
                    pool.reserve0,
                    pool.decimals1,
                    pool.decimals0,
                )
            } else {
                return None;
            };

        Some(Self {
            pool: pool.address,
            token_in,
            token_out,
            fee: pool.fee,
            dex_type: pool.dex_type,
            reserve_in,
            reserve_out,
            decimals_in,
            decimals_out,
            sqrt_price_x96: pool.sqrt_price_x96,
            liquidity: pool.liquidity,
        })
    }

    /// Calcula output amount para um input específico
    pub fn get_amount_out(&self, amount_in: U256) -> U256 {
        match self.dex_type {
            DexType::UniswapV2 => self.get_amount_out_v2(amount_in),
            DexType::Aerodrome => {
                // Aerodrome pode ser V2 ou Stable
                // Aqui assumimos V2 - para Stable precisaríamos de flag adicional
                self.get_amount_out_v2(amount_in)
            }
            DexType::UniswapV3 => self.get_amount_out_v3(amount_in),
            DexType::AerodromeStable => self.get_amount_out_stable(amount_in),
            DexType::PancakeSwap => self.get_amount_out_v3(amount_in), // PancakeSwap V3 usa mesma matemática
        }
    }

    /// Fórmula V2/Aerodrome vAMM:  dy = (dx·f_num · y) / (x·f_den + dx·f_num)
    ///
    /// O fee é lido de `self.fee` (em basis-points: 30 = 0.30%, 100 = 1.00%).
    /// Forma canónica Uniswap V2:  f_num = (10_000 − fee), f_den = 10_000.
    ///
    /// Todos os intermediários em U256 — sem risco de overflow para reserves
    /// realistas na Base (tipicamente < 10^24 wei, u256::MAX ≈ 1.15 × 10^77).
    fn get_amount_out_v2(&self, amount_in: U256) -> U256 {
        if self.reserve_in.is_zero() || self.reserve_out.is_zero() || amount_in.is_zero() {
            return U256::ZERO;
        }

        // fee em bps (ex: 30 = 0.30%).  Usar a fee real do pool em vez de
        // hardcoded 997/1000, que equivalia sempre a 0.30% independentemente
        // do pool.  Aerodrome vAMM usa 30 bps; BaseSwap usa 25 bps; etc.
        //
        // f_num = 10_000 − fee   →  para fee=30:  9_970
        // f_den = 10_000         →  equivalente ao 1000 da fórmula original
        let fee_bps = u64::from(self.fee.min(9_999)); // clamp para evitar subtracção com underflow
        let f_num = U256::from(10_000u64 - fee_bps);
        let f_den = U256::from(10_000u64);

        // Todos os intermediários em U256 — sem overflow
        let amount_in_with_fee = amount_in * f_num; // dx · f_num  (sem divisão aqui)
        let numerator = amount_in_with_fee * self.reserve_out;
        let denominator = self.reserve_in * f_den + amount_in_with_fee;

        if denominator.is_zero() {
            return U256::ZERO;
        }

        let result = numerator / denominator;

        trace!(
            "[V2-MATH] in={} r_in={} r_out={} fee={}bps → out={}",
            amount_in,
            self.reserve_in,
            self.reserve_out,
            fee_bps,
            result
        );

        // Sanidade: resultado não pode exceder reserve_out
        debug_assert!(
            result <= self.reserve_out,
            "get_amount_out_v2: resultado {} excede reserve_out {}",
            result,
            self.reserve_out
        );

        result
    }

    /// Fórmula Stable: x³y + xy³ = k
    /// 🚨 CORREÇÃO: Usa math::stable com u128 anti-overflow
    fn get_amount_out_stable(&self, amount_in: U256) -> U256 {
        if self.reserve_in.is_zero() || self.reserve_out.is_zero() {
            return U256::ZERO;
        }

        // Converter para u128 (valores normalizados cabem em u128)
        let ri: u128 = self.reserve_in.try_into().unwrap_or(u128::MAX);
        let ro: u128 = self.reserve_out.try_into().unwrap_or(u128::MAX);
        let ai: u128 = amount_in.try_into().unwrap_or(u128::MAX);

        // Usar nova implementação anti-overflow
        use crate::math::stable::get_amount_out_stable;
        match get_amount_out_stable(ai, ri, ro, self.decimals_in, self.decimals_out) {
            Some(out) => U256::from(out),
            None => U256::ZERO,
        }
    }

    /// V3: cálculo exacto usando sqrt_price_x96 e liquidez (TickMath)
    ///
    /// Fórmula (dentro do tick activo):
    ///   x_virtual = L * 2^96 / sqrt_price_x96  (token0)
    ///   y_virtual = L * sqrt_price_x96 / 2^96  (token1)
    ///
    /// IMPORTANTE: self.reserve_in/reserve_out já estão na ordem correta do swap
    /// (definidos em from_pool baseado em token_in == token0 ou token1).
    /// NÃO recalcular x_virtual/y_virtual fixos — usar as reserves virtuais já orientadas.
    fn get_amount_out_v3(&self, amount_in: U256) -> U256 {
        // Verificar se temos dados V3 válidos
        if self.liquidity.is_none() || self.sqrt_price_x96.is_none() {
            return self.get_amount_out_v2_approx(amount_in);
        }

        // self.reserve_in/reserve_out já contêm as virtual reserves na ordem correta
        // (from_pool trocou se token_in == token1 da pool)
        if self.reserve_in.is_zero() || self.reserve_out.is_zero() || amount_in.is_zero() {
            return U256::ZERO;
        }

        // Usar math::v3 com as reserves virtuais já orientadas pelo swap direction
        match crate::math::v3::get_amount_out(amount_in, self.reserve_in, self.reserve_out, self.fee) {
            Some(out) => out,
            None => U256::ZERO,
        }
    }

    /// Aproximação V2 usada como fallback para V3 quando tick data não está disponível.
    /// Usa fee do pool em ppm (Uniswap V3: 500=0.05%, 3000=0.3%, 10000=1%).
    fn get_amount_out_v2_approx(&self, amount_in: U256) -> U256 {
        if self.reserve_in.is_zero() || self.reserve_out.is_zero() || amount_in.is_zero() {
            return U256::ZERO;
        }
        // fee_ppm directamente (V3 usa ppm); escalar para base 10_000 para
        // consistência com a aritmética abaixo: f_num = 10_000 − fee_ppm/100
        // Exemplo: fee=500 ppm → 500/100=5 → f_num=9995 → 0.05% efectivo ✓
        let fee_ppm = self.fee.min(999_999);
        let fee_bps_approx = fee_ppm / 100; // 500 ppm → 5 bps (aproximação)
        let f_num = U256::from(10_000u64 - u64::from(fee_bps_approx.min(9_999)));
        let amount_in_with_fee = amount_in * f_num / U256::from(10_000u64);
        let numerator = amount_in_with_fee * self.reserve_out;
        let denominator = self.reserve_in + amount_in_with_fee;
        if denominator.is_zero() {
            U256::ZERO
        } else {
            numerator / denominator
        }
    }

    fn normalize(&self, amount: U256, decimals: u8) -> U256 {
        if decimals == 18 {
            amount
        } else if decimals < 18 {
            amount * U256::from(10).pow(U256::from(18 - decimals))
        } else {
            amount / U256::from(10).pow(U256::from(decimals - 18))
        }
    }

    fn denormalize(&self, amount: U256, decimals: u8) -> U256 {
        if decimals == 18 {
            amount
        } else if decimals < 18 {
            amount / U256::from(10).pow(U256::from(18 - decimals))
        } else {
            amount * U256::from(10).pow(U256::from(decimals - 18))
        }
    }
}

/// Caminho de arbitragem completo
#[derive(Clone, Debug)]
pub struct ArbPath {
    pub hops: SmallVec<[Edge; MAX_PATH_HOPS]>,
    pub start_token: Address,
    pub input_amount: U256,
    pub output_amount: U256,
    /// Lucro bruto em wei
    pub gross_profit: U256,
    /// Custo estimado de gas em wei
    pub gas_cost: U256,
    /// Lucro líquido (gross_profit - gas_cost)
    pub net_profit: U256,
    /// Profit-to-gas ratio
    pub profit_ratio: f64,
    /// Flash loan fee pago (em wei)
    pub flash_loan_fee: U256,
}

impl ArbPath {
    /// Verifica se o path é um ciclo válido (volta ao token inicial)
    pub fn is_valid_cycle(&self) -> bool {
        if self.hops.is_empty() {
            return false;
        }
        let last_token = self.hops.last().unwrap().token_out;
        last_token == self.start_token
    }

    /// Converte para Hop[] usado pelo executor
    pub fn to_hops(&self) -> Vec<Hop> {
        self.hops
            .iter()
            .map(|edge| Hop {
                pool: edge.pool,
                token_in: edge.token_in,
                token_out: edge.token_out,
                fee: edge.fee,
                dex_type: edge.dex_type,
            })
            .collect()
    }

    /// Calcula identificador único para deduplicação
    pub fn unique_id(&self) -> String {
        let pool_ids: Vec<String> = self.hops.iter().map(|e| format!("{:?}", e.pool)).collect();
        pool_ids.join("-")
    }
}

/// 📊 Estatísticas do grafo para debugging
#[derive(Clone, Debug)]
pub struct GraphStats {
    pub pool_count: usize,
    pub token_count: usize,
    pub edge_count: usize,
    pub last_rebuild_block: u64,
}

/// Grafo de arbitragem
#[derive(Debug)]
pub struct ArbGraph {
    /// Cache de pools
    pool_cache: PoolCache,
    /// Mapeamento token -> edges saindo deste token
    adjacency: HashMap<Address, Vec<Edge>>,
    /// Índice invertido: token -> pools que o contêm (O(1) lookup)
    token_to_pools: HashMap<Address, Vec<Address>>,
    /// Set de tokens monitorizados
    tokens: HashSet<Address>,
    /// Limite mínimo de TVL em ETH para incluir pool
    min_tvl_eth: U256,
    /// Bloco da última reconstrução
    last_rebuild_block: u64,
}

impl ArbGraph {
    pub fn new(pool_cache: PoolCache, min_tvl_eth: U256) -> Self {
        Self {
            pool_cache,
            adjacency: HashMap::new(),
            token_to_pools: HashMap::new(),
            tokens: HashSet::new(),
            min_tvl_eth,
            last_rebuild_block: 0,
        }
    }

    /// Reconstrói grafo a partir do cache (chamar após cada bloco)
    pub fn rebuild(&mut self, current_block: u64) {
        if current_block == self.last_rebuild_block && self.last_rebuild_block != 0 {
            return;
        }
        self.adjacency.clear();
        self.token_to_pools.clear();
        self.tokens.clear();

        // Bug C: snapshot total before filtering
        let total_in_cache = self.pool_cache.len();

        // Bug D: use U256::ZERO as the effective TVL floor so that pools whose
        // reserve0 is a low-decimal stablecoin (e.g. USDC with 6 decimals, giving
        // ~10^12 for 1M USDC) are not wrongly excluded by a 10^19 ETH threshold.
        // has_liquidity() remains the real gatekeeper for non-empty reserves.
        let all_pools = self.pool_cache.get_active_pools(U256::ZERO);
        let passing_tvl = all_pools.len();

        // Bug C: separate the two filter stages so we can count each independently
        let mut filtered_no_liquidity = 0usize;
        let mut filtered_stale = 0usize;

        for pool in all_pools {
            // Stage 1: must have non-zero reserves
            if !pool.has_liquidity() {
                filtered_no_liquidity += 1;
                continue;
            }
            // Stage 2: data must not be stale
            if pool.is_stale(current_block) {
                filtered_stale += 1;
                continue;
            }

            // Criar edges bidirecionais
            self.add_pool_edges(&pool);

            if pool.token0 != Address::ZERO { self.tokens.insert(pool.token0); }
            if pool.token1 != Address::ZERO { self.tokens.insert(pool.token1); }
        }

        self.last_rebuild_block = current_block;

        // Bug C: log the full filter pipeline + WETH edge count
        let weth = address!("4200000000000000000000000000000000000006");
        let weth_edges = self.adjacency.get(&weth).map(|v| v.len()).unwrap_or(0);
        info!(
            "🕸️ [ArbGraph] Rebuild | Cache: {} | Passing TVL: {} | No-liquidity: {} | Stale: {} | WETH edges: {}",
            total_in_cache, passing_tvl, filtered_no_liquidity, filtered_stale, weth_edges
        );

        debug!(
            "🕸️ [ArbGraph] Rebuilt | Tokens: {} | Edges: {} | Block: {}",
            self.tokens.len(),
            self.adjacency.values().map(|v| v.len()).sum::<usize>(),
            current_block
        );

        if self.tokens.len() <= 3 {
            trace!("[GRAPH] Tokens: {:?}", self.tokens);
        }
        if self.tokens.contains(&Address::ZERO) {
            warn!("[GRAPH] Detetado Address::ZERO em tokens — pools podem não ter token0/token1 preenchidos");
        }

        // DIAGNÓSTICO: Log de todas as edges
        for (token_from, edges) in &self.adjacency {
            for edge in edges {
                debug!(
                    "[GRAPH-EDGE] {:?} -> {:?} via pool {:?}",
                    token_from, edge.token_out, edge.pool
                );
            }
        }
    }

    fn add_pool_edges(&mut self, pool: &PoolState) {
        if pool.token0 == Address::ZERO || pool.token1 == Address::ZERO { return; }
        self.token_to_pools.entry(pool.token0).or_default().push(pool.address);
        self.token_to_pools.entry(pool.token1).or_default().push(pool.address);
        if let Some(edge0) = Edge::from_pool(pool, pool.token0) {
            self.adjacency.entry(pool.token0).or_default().push(edge0);
        }
        if let Some(edge1) = Edge::from_pool(pool, pool.token1) {
            self.adjacency.entry(pool.token1).or_default().push(edge1);
        }
    }

    /// 📊 Estatísticas do grafo para logging transparente
    pub fn get_stats(&self) -> GraphStats {
        let edge_count: usize = self.adjacency.values().map(|v| v.len()).sum();
        GraphStats {
            pool_count: self.pool_cache.len(),
            token_count: self.tokens.len(),
            edge_count,
            last_rebuild_block: self.last_rebuild_block,
        }
    }

    /// Encontra todos os ciclos 2-hop (A -> B -> A)
    pub fn find_2hop_cycles(&self, start_token: Address) -> Vec<ArbPath> {
        let mut cycles = Vec::new();

        let start_edges = match self.adjacency.get(&start_token) {
            Some(edges) => edges,
            None => return cycles,
        };

        for edge1 in start_edges {
            let mid_token = edge1.token_out;
            if mid_token == start_token { continue; } // skip loops degenerados

            // Procurar caminho de volta
            if let Some(return_edges) = self.adjacency.get(&mid_token) {
                for edge2 in return_edges {
                    if edge2.pool == edge1.pool { continue; } // nunca lucro no mesmo pool
                    if edge2.token_out == start_token {
                        let path = ArbPath {
                            hops: SmallVec::from_slice(&[edge1.clone(), edge2.clone()]),
                            start_token,
                            input_amount: U256::ZERO,
                            output_amount: U256::ZERO,
                            gross_profit: U256::ZERO,
                            gas_cost: U256::ZERO,
                            net_profit: U256::ZERO,
                            profit_ratio: 0.0,
                            flash_loan_fee: U256::ZERO,
                        };
                        cycles.push(path);
                    }
                }
            }
        }

        cycles
    }

    /// Encontra todos os ciclos 3-hop triangulares (A -> B -> C -> A)
    /// Encontra todos os ciclos 3-hop triangulares (A -> B -> C -> A)
    /// Optimizado: guards extra eliminam ramos inúteis antes do 3º loop
    pub fn find_3hop_cycles(&self, start_token: Address) -> Vec<ArbPath> {
        let mut cycles = Vec::new();
        let mut seen_paths = HashSet::new();

        // Pre-computar set de tokens que fecham directamente para start_token
        let closes_to_start: HashSet<Address> = self.adjacency
            .iter()
            .filter(|(_, edges)| edges.iter().any(|e| e.token_out == start_token))
            .map(|(token, _)| *token)
            .collect();

        let start_edges = match self.adjacency.get(&start_token) {
            Some(edges) => edges,
            None => return cycles,
        };

        for edge1 in start_edges {
            let mid_token1 = edge1.token_out;
            if mid_token1 == start_token { continue; }
            let mid_edges = match self.adjacency.get(&mid_token1) {
                Some(edges) => edges,
                None => continue,
            };
            for edge2 in mid_edges {
                if edge2.pool == edge1.pool { continue; }
                let mid_token2 = edge2.token_out;
                if mid_token2 == start_token || mid_token2 == mid_token1 { continue; }
                if !closes_to_start.contains(&mid_token2) { continue; }
                let final_edges = match self.adjacency.get(&mid_token2) {
                    Some(edges) => edges,
                    None => continue,
                };
                for edge3 in final_edges {
                    if edge3.pool == edge1.pool || edge3.pool == edge2.pool { continue; }
                    if edge3.token_out != start_token { continue; }
                    let path = ArbPath {
                        hops: SmallVec::from_slice(&[*edge1, *edge2, *edge3]),
                        start_token,
                        input_amount: U256::ZERO,
                        output_amount: U256::ZERO,
                        gross_profit: U256::ZERO,
                        gas_cost: U256::ZERO,
                        net_profit: U256::ZERO,
                        profit_ratio: 0.0,
                        flash_loan_fee: U256::ZERO,
                    };
                    let id = path.unique_id();
                    if seen_paths.insert(id) {
                        cycles.push(path);
                    }
                }
            }
        }
        cycles
    }
    /// Encontra todas as oportunidades a partir de um token
    /// 🚨 CORREÇÃO: Logging detalhado para diagnóstico
    pub fn find_opportunities(
        &self,
        start_token: Address,
        flash_loan_amounts: &[U256],
        gas_price_wei: U256,
        min_profit_ratio: f64,
    ) -> Vec<ArbPath> {
        self.find_opportunities_with_priorities(
            start_token,
            flash_loan_amounts,
            gas_price_wei,
            min_profit_ratio,
            None,
        )
    }

    pub fn find_opportunities_with_priorities(
        &self,
        start_token: Address,
        flash_loan_amounts: &[U256],
        gas_price_wei: U256,
        min_profit_ratio: f64,
        pool_priorities: Option<&HashMap<Address, f64>>,
    ) -> Vec<ArbPath> {
        // 🔬 DIAGNÓSTICO: Contar pools com liquidez
        let cache_stats = self.pool_cache.stats();

        tracing::debug!(
            "[GRAPH] Pools com liquidez: {}/{} total",
            cache_stats.active_pools,
            cache_stats.total_pools
        );

        let mut all_paths = Vec::new();

        // Coletar ciclos 2-hop e 3-hop
        let cycles_2hop = self.find_2hop_cycles(start_token);
        let cycles_3hop = self.find_3hop_cycles(start_token);

        all_paths.extend(cycles_2hop);
        all_paths.extend(cycles_3hop);

        if let Some(priorities) = pool_priorities {
            all_paths.sort_by(|a, b| {
                let score = |path: &ArbPath| -> f64 {
                    path.hops
                        .iter()
                        .map(|hop| priorities.get(&hop.pool).copied().unwrap_or(0.0))
                        .sum::<f64>()
                };
                score(b)
                    .partial_cmp(&score(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        tracing::info!(
            "[GRAPH] Ciclos encontrados: {} (2-hop + 3-hop)",
            all_paths.len()
        );

        // Calcular lucro para cada ciclo com diferentes amounts
        let mut profitable_paths = Vec::new();

        // 🔬 Contadores de diagnóstico
        let mut paths_checked = 0usize;
        let mut filtered_zero_output = 0usize; // Bug A
        let mut filtered_no_profit = 0usize;
        let mut filtered_gas = 0usize;
        let mut filtered_ratio = 0usize;
        let mut valid = 0usize;

        // Bug B: inline closure that builds a human-readable token path string, e.g.
        //   WETH→USDC→DAI→WETH
        // Uses {:?} formatting for each address (gives the checksum hex form).
        let make_path_str = |hops: &SmallVec<[Edge; MAX_PATH_HOPS]>| -> String {
            let mut parts: Vec<String> = Vec::with_capacity(hops.len() + 1);
            if let Some(first) = hops.first() {
                parts.push(format!("{:?}", first.token_in));
            }
            for hop in hops.iter() {
                parts.push(format!("{:?}", hop.token_out));
            }
            parts.join("→")
        };

        for mut path in all_paths {
            // Bug B: compute path label once per path (token structure never changes)
            let path_label = make_path_str(&path.hops);

            for amount_in in flash_loan_amounts {
                paths_checked += 1;
                let mut amount_out = *amount_in;
                let mut path_valid = true;
                let mut zero_hop_idx = 0usize;

                trace!("[PATH] {} | input: {}", path_label, amount_in);

                // Bug B: simulate hops with per-hop trace logging
                for (hop_idx, edge) in path.hops.iter().enumerate() {
                    let hop_out = edge.get_amount_out(amount_out);
                    trace!(
                        "[PATH] {} | hop-{}: {} → {}",
                        path_label,
                        hop_idx,
                        amount_out,
                        hop_out
                    );
                    amount_out = hop_out;
                    if amount_out.is_zero() {
                        zero_hop_idx = hop_idx;
                        path_valid = false;

                        // Debug V3 zero-output: logar reserves do hop que falhou
                        if edge.dex_type == DexType::UniswapV3 {
                            tracing::warn!(
                                path = %path_label,
                                hop = hop_idx,
                                amount_in = %amount_out, // input deste hop que deu zero
                                pool = %edge.pool,
                                reserve_in = %edge.reserve_in,
                                reserve_out = %edge.reserve_out,
                                sqrt_price_x96 = ?edge.sqrt_price_x96,
                                liquidity = ?edge.liquidity,
                                "V3 path zero-output — dumping edge state"
                            );
                        }
                        break;
                    }
                }

                if !path_valid {
                    trace!(
                        "[PATH] {} | rejected: zero-output@hop-{}",
                        path_label,
                        zero_hop_idx
                    );
                    filtered_zero_output += 1; // Bug A
                    continue;
                }

                // Calcular lucro
                let gross_profit = if amount_out > *amount_in {
                    amount_out - *amount_in
                } else {
                    trace!(
                        "[PATH] {} | rejected: no-profit (out={} in={})",
                        path_label,
                        amount_out,
                        amount_in
                    );
                    filtered_no_profit += 1;
                    continue; // Sem lucro
                };

                // Estimar gas (simplificado)
                let gas_used = 120_000 + (path.hops.len() as u64 * 40_000);
                let gas_cost = gas_price_wei * U256::from(gas_used);

                let has_v3 = path.hops.iter().any(|p| matches!(p.dex_type, DexType::UniswapV3));
                if has_v3 {
                    tracing::debug!(
                        path_label = %path_label,
                        amount_in = %amount_in,
                        amount_out = %amount_out,
                        gross_profit = %gross_profit,
                        gas_cost = %gas_cost,
                        "cross-dex V3 path"
                    );
                }

                if gas_cost >= gross_profit {
                    trace!(
                        "[PATH] {} | rejected: gas-high (cost={} profit={})",
                        path_label,
                        gas_cost,
                        gross_profit
                    );
                    filtered_gas += 1;
                    continue; // Não cobre gas
                }

                // Flash loan fee: Balancer=0bps, Aave=9bps
                // Usar Balancer (0%) por defeito — fee=0 não afecta profit
                // mas está disponível para comparação com outros providers
                let flash_fee_bps = U256::from(0u64); // 0 = Balancer V2 gratuito
                let flash_loan_fee = *amount_in * flash_fee_bps / U256::from(10_000u64);
                let net_profit = if gross_profit > gas_cost + flash_loan_fee {
                    gross_profit - gas_cost - flash_loan_fee
                } else {
                    U256::ZERO
                };
                let profit_ratio = if gas_cost.is_zero() { f64::MAX } else {
                    gross_profit.try_into().unwrap_or(u128::MAX) as f64 / gas_cost.try_into().unwrap_or(u128::MAX) as f64
                };
                if profit_ratio < min_profit_ratio {
                    trace!(
                        "[PATH] {} | rejected: ratio-low ({:.4} < {:.4})",
                        path_label,
                        profit_ratio,
                        min_profit_ratio
                    );
                    filtered_ratio += 1;
                    continue;
                }

                // Atualizar path com valores calculados
                path.input_amount = *amount_in;
                path.output_amount = amount_out;
                path.gross_profit = gross_profit;
                path.gas_cost = gas_cost;
                path.net_profit = net_profit;
                path.profit_ratio = profit_ratio;
                path.flash_loan_fee = flash_loan_fee;

                valid += 1;
                profitable_paths.push(path.clone());
            }
        }

        // 🔬 DIAGNÓSTICO FINAL
        tracing::info!(
            "[GRAPH] Paths: {} checked | {} zero-output | {} no-profit | {} gas-high | {} ratio-low | ✅ {} valid",
            paths_checked,
            filtered_zero_output,
            filtered_no_profit,
            filtered_gas,
            filtered_ratio,
            valid
        );

        // Ordenar por lucro líquido decrescente
        profitable_paths.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));

        // Remover duplicados (mesmo path com amounts diferentes, manter melhor)
        let mut unique_best: HashMap<String, ArbPath> = HashMap::new();
        for path in profitable_paths {
            let key = path.unique_id();
            unique_best.entry(key).and_modify(|existing| { if path.net_profit > existing.net_profit { *existing = path.clone(); } }).or_insert(path);
        }

        unique_best.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_edge_v2_calculation() {
        let edge = Edge {
            pool: address!("0x1234567890123456789012345678901234567890"),
            token_in: address!("0x4200000000000000000000000000000000000006"),
            token_out: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            fee: 3000,
            dex_type: DexType::UniswapV2,
            reserve_in: U256::from(100000000000000000000u128), // 100 ETH
            reserve_out: U256::from(300000000000u128),         // 300k USDC
            decimals_in: 18,
            decimals_out: 6,
            sqrt_price_x96: None,
            liquidity: None,
        };

        let amount_in = U256::from(1000000000000000000u128); // 1 ETH
        let out = edge.get_amount_out(amount_in);

        // Deve retornar ~2970 USDC (com fee 0.3%)
        assert!(out > U256::ZERO);
    }

    #[test]
    fn test_arb_path_validation() {
        let edge1 = Edge {
            pool: address!("0x1111111111111111111111111111111111111111"),
            token_in: address!("0x4200000000000000000000000000000000000006"),
            token_out: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            fee: 3000,
            dex_type: DexType::UniswapV2,
            reserve_in: U256::from(100),
            reserve_out: U256::from(300),
            decimals_in: 18,
            decimals_out: 6,
            sqrt_price_x96: None,
            liquidity: None,
        };

        let edge2 = Edge {
            pool: address!("0x2222222222222222222222222222222222222222"),
            token_in: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            token_out: address!("0x4200000000000000000000000000000000000006"),
            fee: 3000,
            dex_type: DexType::UniswapV2,
            reserve_in: U256::from(300),
            reserve_out: U256::from(100),
            decimals_in: 6,
            decimals_out: 18,
            sqrt_price_x96: None,
            liquidity: None,
        };

        let path = ArbPath {
            hops: SmallVec::from_slice(&[edge1, edge2]),
            start_token: address!("0x4200000000000000000000000000000000000006"),
            input_amount: U256::ZERO,
            output_amount: U256::ZERO,
            gross_profit: U256::ZERO,
            gas_cost: U256::ZERO,
            net_profit: U256::ZERO,
            profit_ratio: 0.0,
        };

        assert!(path.is_valid_cycle());
    }
}
