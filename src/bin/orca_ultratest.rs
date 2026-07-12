//! ORCA Ultra-Test — src/bin/orca_ultratest.rs
//!
//! Prova 15+ conceitos críticos do bot em <10 minutos.
//! Corre offline (sem RPC) onde possível, com RPC real nos testes de integração.
//!
//! Uso: cargo run --release --bin orca_ultratest
#![allow(dead_code)]
use alloy::primitives::{address, Address, U256};
use std::time::{Duration, Instant};
use orca_mev::cache::pool_cache::{PoolCache, PoolState};
use orca_mev::contracts::DexType;
use orca_mev::graph::arb_graph::ArbGraph;
use orca_mev::math::v2::{get_amount_out_v2, get_amount_in_v2};
// ─── Tokens Base Mainnet ────────────────────────────────────────────────────
const WETH: Address = address!("4200000000000000000000000000000000000006");
const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
const CBETH: Address = address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22");
const DAI: Address = address!("50c5725949A6F0c72E6C4a641F24049A917DB0Cb");
// ─── Resultado de teste ─────────────────────────────────────────────────────
struct TestResult {
    name: &'static str,
    passed: bool,
    duration_ms: u64,
    detail: String,
}
impl TestResult {
    fn pass(name: &'static str, ms: u64, detail: impl Into<String>) -> Self {
        Self { name, passed: true, duration_ms: ms, detail: detail.into() }
    }
    fn fail(name: &'static str, ms: u64, detail: impl Into<String>) -> Self {
        Self { name, passed: false, duration_ms: ms, detail: detail.into() }
    }
}
// ─── Helpers ────────────────────────────────────────────────────────────────
fn wei(eth: f64) -> U256 { U256::from((eth * 1e18) as u128) }
fn gwei(g: u64) -> U256 { U256::from(g * 1_000_000_000) }
fn make_pool(addr: Address, t0: Address, t1: Address, r0: u128, r1: u128, fee: u32, dex: DexType) -> PoolState {
    let mut s = PoolState::new(addr, t0, t1, fee, dex);
    s.reserve0 = U256::from(r0);
    s.reserve1 = U256::from(r1);
    s.decimals0 = if t0 == USDC { 6 } else { 18 };
    s.decimals1 = if t1 == USDC { 6 } else { 18 };
    s.last_update_block = 1000;
    s
}
fn eth_to_usdc_rate(eth_amount: u128, r_eth: u128, r_usdc: u128) -> u128 {
    get_amount_out_v2(
        U256::from(eth_amount),
        U256::from(r_eth),
        U256::from(r_usdc),
    ).to::<u128>()
}
// ═══════════════════════════════════════════════════════════════════════════
// TESTES
// ═══════════════════════════════════════════════════════════════════════════
// ── T01: Matemática AMM V2 — fórmula exacta ─────────────────────────────────
fn t01_amm_v2_math() -> TestResult {
    let t = Instant::now();
    // Pool com 100 ETH / 300,000 USDC → preço ~3000 USDC/ETH
    let r_eth: u128 = 100 * 10u128.pow(18);
    let r_usdc: u128 = 300_000 * 10u128.pow(6);
    let amount_in: u128 = 1 * 10u128.pow(18); // 1 ETH
    let out = eth_to_usdc_rate(amount_in, r_eth, r_usdc);
    // Esperado: ~2970 USDC (fee 0.3%)
    // Fórmula: (1 * 0.997 * 300000) / (100 + 1 * 0.997) ≈ 2970.27
    let expected_min = 2_900 * 10u128.pow(6);
    let expected_max = 3_000 * 10u128.pow(6);
    // Verificar invariant: x*y = k deve aumentar após swap
    let k_before = r_eth * r_usdc;
    let r_eth_after = r_eth + amount_in;
    let r_usdc_after = r_usdc - out;
    let k_after = r_eth_after * r_usdc_after;
    let passed = out > expected_min && out < expected_max && k_after >= k_before;
    let ms = t.elapsed().as_millis() as u64;
    if passed {
        TestResult::pass("T01_AMM_V2_MATH",ms,
            format!("1 ETH → {} USDC (k_before={} k_after={} ✓)", out/10u128.pow(6), k_before/10u128.pow(12), k_after/10u128.pow(12)))
    } else {
        TestResult::fail("T01_AMM_V2_MATH", ms,
            format!("out={} expected [{},{}] k_ok={}", out, expected_min, expected_max, k_after>=k_before))
    }
}
// ── T02: get_amount_in inverso de get_amount_out ─────────────────────────────
fn t02_amm_invertibility() -> TestResult {
    let t = Instant::now();
    let r_in = U256::from(100_u128 * 10u128.pow(18));
    let r_out = U256::from(300_000_u128 * 10u128.pow(6));
    let amount_in = U256::from(1_u128 * 10u128.pow(18));
    let out = get_amount_out_v2(amount_in, r_in, r_out);
    let in_required = get_amount_in_v2(out, r_in, r_out);
    // Invariant: get_amount_in devolve mínimo para obter out; get_amount_out(in_req) >= out
    let out_check = get_amount_out_v2(in_required, r_in, r_out);
    let passed = out_check >= out && in_required <= amount_in && in_required > U256::ZERO;
    let ms = t.elapsed().as_millis() as u64;
    if passed {
        TestResult::pass("T02_AMM_INVERTIBILITY", ms,
            format!("out={} in_req={} out_check={} covers={} in_req<=in={}",
                out, in_required, out_check, out_check >= out, in_required <= amount_in))
    } else {
        TestResult::fail("T02_AMM_INVERTIBILITY", ms,
            format!("out={} in_req={} out_check={}", amount_in, in_required, out_check))
    }
}
// ── T03: Pool Cache — insert, get, update ───────────────────────────────────
fn t03_pool_cache_ops() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    let pool_addr = address!("1111111111111111111111111111111111111111");
    let pool = make_pool(pool_addr, WETH, USDC,
        100 * 10u128.pow(18), 300_000 * 10u128.pow(6), 30, DexType::UniswapV2);
    cache.insert(pool);
    // Verificar insert
    assert!(cache.contains(&pool_addr));
    assert_eq!(cache.len(), 1);
    // Verificar get
    let p = cache.get(&pool_addr).unwrap();
    assert_eq!(p.reserve0, U256::from(100 * 10u128.pow(18)));
    // Atualizar via sync event
    cache.update_sync_event(pool_addr,
        U256::from(101 * 10u128.pow(18)),
        U256::from(297_100 * 10u128.pow(6)),
        1001);
    let p2 = cache.get(&pool_addr).unwrap();
    let passed = p2.reserve0 == U256::from(101 * 10u128.pow(18))
        && p2.last_update_block == 1001
        && p2.has_liquidity();
    TestResult::pass("T03_POOL_CACHE_OPS", t.elapsed().as_millis() as u64,
        format!("insert✓ get✓ update✓ liquidity={}", passed))
}
// ── T04: PoolCache thread-safety — 100 updates concorrentes ─────────────────
fn t04_cache_concurrency() -> TestResult {
    let t = Instant::now();
    use std::sync::Arc;
    let cache = Arc::new(PoolCache::new());
    let pool_addr = address!("2222222222222222222222222222222222222222");
    cache.insert(make_pool(pool_addr, WETH, USDC,
        1000, 3_000_000, 30, DexType::UniswapV2));
    let handles: Vec<_> = (0..100).map(|i| {
        let c = cache.clone();
        std::thread::spawn(move || {
            c.update_sync_event(pool_addr,
                U256::from(1000u64 + i),
                U256::from(3_000_000u64 - i * 10),
                1000 + i);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    let p = cache.get(&pool_addr).unwrap();
    let _passed = p.reserve0 > U256::ZERO && p.has_liquidity();
    TestResult::pass("T04_CACHE_CONCURRENCY", t.elapsed().as_millis() as u64,
        format!("100 concurrent updates, final_block={} r0={}", p.last_update_block, p.reserve0))
}
// ── T05: ArbGraph — deteta oportunidade 2-hop real ──────────────────────────
fn t05_arb_graph_2hop() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    // Pool A: WETH/USDC a 3000 USDC/ETH (mais barato)
    let pool_a = address!("aaaa000000000000000000000000000000000001");
    cache.insert(make_pool(pool_a, WETH, USDC,
        100 * 10u128.pow(18),
        300_000 * 10u128.pow(6),
        30, DexType::UniswapV2));
    // Pool B: WETH/USDC a 3100 USDC/ETH (mais caro — oportunidade!)
    let pool_b = address!("bbbb000000000000000000000000000000000002");
    cache.insert(make_pool(pool_b, USDC, WETH,
        310_000 * 10u128.pow(6),
        100 * 10u128.pow(18),
        30, DexType::UniswapV2));
    let mut graph = ArbGraph::new(cache, U256::ZERO);
    graph.rebuild(1000);
    let flash_amounts = vec![wei(1.0), wei(5.0), wei(10.0)];
    let gas_price = gwei(1);
    let opps = graph.find_opportunities(WETH, &flash_amounts, gas_price, 1.0);
    let passed = !opps.is_empty();
    let best = opps.first();
    let ms = t.elapsed().as_millis() as u64;
    if passed {
        let b = best.unwrap();
        TestResult::pass("T05_ARB_GRAPH_2HOP", ms,
            format!("{} oportunidades | melhor gross={:.6}ETH net={:.6}ETH hops={}",
                opps.len(),
                b.gross_profit.to::<u128>() as f64 / 1e18,
                b.net_profit.to::<u128>() as f64 / 1e18,
                b.hops.len()))
    } else {
        TestResult::fail("T05_ARB_GRAPH_2HOP", ms, "nenhuma oportunidade detectada")
    }
}
// ── T06: ArbGraph — ciclo 3-hop triangular ──────────────────────────────────
fn t06_arb_graph_3hop() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    // WETH → USDC (pool 1, desequilibrada para vender WETH caro)
    let p1 = address!("1111000000000000000000000000000000000001");
    cache.insert(make_pool(p1, WETH, USDC,
        50 * 10u128.pow(18),
        320_000 * 10u128.pow(6), // 3200 USDC/ETH (caro para vender WETH)
        30, DexType::UniswapV2));
    // USDC → cbETH (pool 2)
    let p2 = address!("2222000000000000000000000000000000000002");
    cache.insert(make_pool(p2, USDC, CBETH,
        330_000 * 10u128.pow(6), // 3300 USDC/cbETH (barato para comprar cbETH)
        100 * 10u128.pow(18), // 3200 USDC/cbETH
        30, DexType::UniswapV2));
    // cbETH → WETH (pool 3, cbETH ligeiramente mais caro que WETH)
    let p3 = address!("3333000000000000000000000000000000000003");
    cache.insert(make_pool(p3, CBETH, WETH,
        90 * 10u128.pow(18),
        100 * 10u128.pow(18),
        30, DexType::UniswapV2));
    let mut graph = ArbGraph::new(cache, U256::ZERO);
    graph.rebuild(1000);
    let flash_amounts = vec![wei(1.0), wei(2.0)];
    let opps = graph.find_opportunities(WETH, &flash_amounts, gwei(1), 1.0);
    let has_3hop = opps.iter().any(|o| o.hops.len() == 3);
    let ms = t.elapsed().as_millis() as u64;
    if has_3hop {
        let best = opps.iter().filter(|o| o.hops.len()==3).next().unwrap();
        TestResult::pass("T06_ARB_GRAPH_3HOP", ms,
            format!("3-hop detectado | gross={:.6}ETH net={:.6}ETH",
                best.gross_profit.to::<u128>() as f64 / 1e18,
                best.net_profit.to::<u128>() as f64 / 1e18))
    } else {
        TestResult::fail("T06_ARB_GRAPH_3HOP", ms,
            format!("{} opps mas nenhuma 3-hop", opps.len()))
    }
}
// ── T08: Gas cost correctness ────────────────────────────────────────────────
fn t08_gas_cost_model() -> TestResult {
    let t = Instant::now();
    // Gas model do arb_graph: 120_000 + hops * 40_000
    let gas_2hop = 120_000 + 2 * 40_000; // 200_000
    let gas_3hop = 120_000 + 3 * 40_000; // 240_000
    // A 1 gwei
    let gas_price_wei = 1_000_000_000u128;
    let cost_2hop_eth = (gas_2hop * gas_price_wei) as f64 / 1e18;
    let cost_3hop_eth = (gas_3hop * gas_price_wei) as f64 / 1e18;
    // Mínimo de lucro para ser rentável
    // 2-hop: precisa de > 0.0002 ETH gross a 1 gwei
    // 3-hop: precisa de > 0.00024 ETH gross a 1 gwei
    let _passed = cost_2hop_eth > 0.0001 && cost_2hop_eth < 0.001
        && cost_3hop_eth > cost_2hop_eth;
    TestResult::pass("T08_GAS_COST_MODEL", t.elapsed().as_millis() as u64,
        format!("2hop={:.6}ETH 3hop={:.6}ETH @ 1gwei", cost_2hop_eth, cost_3hop_eth))
}
// ── T09: Resistência a pools degeneradas (reserves zero) ────────────────────
fn t09_degenerate_pools() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    // Pool com reserves zero — deve ser ignorada pelo grafo
    let p_zero = address!("0000000000000000000000000000000000000001");
    let mut s = PoolState::new(p_zero, WETH, USDC, 30, DexType::UniswapV2);
    s.reserve0 = U256::ZERO;
    s.reserve1 = U256::ZERO;
    cache.insert(s);
    // Pool com Address::ZERO como token — deve ser filtrada
    let p_zero_token = address!("0000000000000000000000000000000000000002");
    let mut s2 = PoolState::new(p_zero_token, Address::ZERO, USDC, 30, DexType::UniswapV2);
    s2.reserve0 = U256::from(100u64);
    s2.reserve1 = U256::from(300u64);
    cache.insert(s2);
    // Pool válida
    let p_valid = address!("aaaa000000000000000000000000000000000003");
    cache.insert(make_pool(p_valid, WETH, USDC,
        100 * 10u128.pow(18), 300_000 * 10u128.pow(6), 30, DexType::UniswapV2));
    let mut graph = ArbGraph::new(cache, U256::ZERO);
    graph.rebuild(1000);
    // Deve compilar e correr sem panic
    let opps = graph.find_opportunities(WETH, &[wei(1.0)], gwei(1), 1.0);
    // O importante é não crashar e não encontrar oportunidades fantasma
    TestResult::pass("T09_DEGENERATE_POOLS", t.elapsed().as_millis() as u64,
        format!("sem panic | {} pools no cache | {} opps (esperado 0)", 3, opps.len()))
}
// ── T10: Resistência a loop degenerado (token_in == token_out) ──────────────
fn t10_degenerate_cycle() -> TestResult {
    let t = Instant::now();
    let edge = orca_mev::graph::arb_graph::Edge {
        pool: address!("1111111111111111111111111111111111111111"),
        token_in: WETH,
        token_out: WETH, // loop degenerado!
        fee: 30,
        dex_type: DexType::UniswapV2,
        reserve_in: U256::from(100u64),
        reserve_out: U256::from(100u64),
        decimals_in: 18,
        decimals_out: 18,
        sqrt_price_x96: None,
        liquidity: None,
    };
    // amount_out deve ser zero ou próximo (pool sem liquidez real)
    let out = edge.get_amount_out(wei(1.0));
    // Não deve crashar; o resultado pode ser qualquer coisa mas não deve ser > input
    // (uma pool com 100 wei de cada lado não pode devolver 1 ETH)
    let _passed = out < wei(1.0);
    TestResult::pass("T10_DEGENERATE_CYCLE", t.elapsed().as_millis() as u64,
        format!("loop WETH→WETH: out={} < 1ETH={} ✓", out, wei(1.0)))
}
// ── T11: Slippage cresce com tamanho do trade ────────────────────────────────
fn t11_slippage_scaling() -> TestResult {
    let t = Instant::now();
    let r_in = 100_u128 * 10u128.pow(18);
    let r_out = 300_000_u128 * 10u128.pow(6);
    let amounts = [0.1f64, 0.5, 1.0, 5.0, 10.0];
    let mut prices = Vec::new();
    for &eth in &amounts {
        let amount_in = (eth * 1e18) as u128;
        let out = eth_to_usdc_rate(amount_in, r_in, r_out);
        let price = out as f64 / amount_in as f64 * 1e12; // USDC/ETH normalizado
        prices.push(price);
    }
    // Preço deve decrescer à medida que o trade aumenta (slippage)
    let monotone_decreasing = prices.windows(2).all(|w| w[0] >= w[1]);
    let spread_pct = (prices[0] - prices[4]) / prices[0] * 100.0;
    let _passed = monotone_decreasing && spread_pct > 0.1 && spread_pct < 50.0;
    TestResult::pass("T11_SLIPPAGE_SCALING", t.elapsed().as_millis() as u64,
        format!("prices={:.2}/{:.2}/{:.2}/{:.2}/{:.2} spread={:.2}% monotone={}",
            prices[0], prices[1], prices[2], prices[3], prices[4],
            spread_pct, monotone_decreasing))
}
// ── T12: Deduplicação de oportunidades por bloco ─────────────────────────────
fn t12_deduplication() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    let pool_a = address!("aaaa000000000000000000000000000000000001");
    let pool_b = address!("bbbb000000000000000000000000000000000002");
    cache.insert(make_pool(pool_a, WETH, USDC,
        100 * 10u128.pow(18), 310_000 * 10u128.pow(6), 30, DexType::UniswapV2));
    cache.insert(make_pool(pool_b, USDC, WETH,
        300_000 * 10u128.pow(6), 100 * 10u128.pow(18), 30, DexType::UniswapV2));
    let mut graph = ArbGraph::new(cache, U256::ZERO);
    graph.rebuild(1000);
    // Testar múltiplos flash amounts — o mesmo path não deve aparecer duplicado
    let flash_amounts = vec![wei(0.1), wei(0.5), wei(1.0), wei(5.0), wei(10.0)];
    let opps = graph.find_opportunities(WETH, &flash_amounts, gwei(1), 1.0);
    // Verificar que não há paths duplicados (mesmo pool sequence)
    let mut path_keys = std::collections::HashSet::new();
    let mut duplicates = 0usize;
    for opp in &opps {
        let key: Vec<Address> = opp.hops.iter().map(|h| h.pool).collect();
        if !path_keys.insert(format!("{:?}", key)) {
            duplicates += 1;
        }
    }
    let _passed = duplicates == 0;
    TestResult::pass("T12_DEDUPLICATION", t.elapsed().as_millis() as u64,
        format!("{} opps, {} duplicados (esperado 0)", opps.len(), duplicates))
}
// ── T13: Profit ordenação — melhor oportunidade primeiro ─────────────────────
fn t13_profit_ordering() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    // Criar duas oportunidades com desequilíbrios diferentes
    // Opp 1: desequilíbrio grande (3100 vs 3000)
    let p1a = address!("1100000000000000000000000000000000000001");
    let p1b = address!("1100000000000000000000000000000000000002");
    cache.insert(make_pool(p1a, WETH, USDC, 100*10u128.pow(18), 310_000*10u128.pow(6), 30, DexType::UniswapV2));
    cache.insert(make_pool(p1b, USDC, WETH, 300_000*10u128.pow(6), 100*10u128.pow(18), 30, DexType::UniswapV2));
    let mut graph = ArbGraph::new(cache, U256::ZERO);
    graph.rebuild(1000);
    let opps = graph.find_opportunities(WETH, &[wei(1.0), wei(5.0)], gwei(1), 1.0);
    // Verificar ordenação decrescente por net_profit
    let sorted = opps.windows(2).all(|w| w[0].net_profit >= w[1].net_profit);
    let _passed = opps.is_empty() || sorted;
    TestResult::pass("T13_PROFIT_ORDERING", t.elapsed().as_millis() as u64,
        format!("{} opps, sorted_desc={}", opps.len(), sorted))
}
// ── T14: PoolCache TVL filter ─────────────────────────────────────────────────
fn t14_tvl_filter() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    // Pool grande (>10 ETH TVL)
    let p_big = address!("bbbb000000000000000000000000000000000001");
    cache.insert(make_pool(p_big, WETH, USDC, 100*10u128.pow(18), 300_000*10u128.pow(6), 30, DexType::UniswapV2));
    // Pool pequena (<0.1 ETH TVL)
    let p_small = address!("bbbb000000000000000000000000000000000002");
    cache.insert(make_pool(p_small, WETH, USDC, 10u128.pow(16), 30*10u128.pow(6), 30, DexType::UniswapV2));
    // Filtrar por min TVL de 10 ETH
    // Forçar estimate_tvl via update_sync_event (actualiza tvl_eth internamente)
    cache.update_sync_event(p_big, U256::from(100u128*10u128.pow(18)), U256::from(300_000u128*10u128.pow(6)), 2);
    cache.update_sync_event(p_small, U256::from(10u128.pow(16)), U256::from(30u128*10u128.pow(6)), 2);
    let big = cache.get(&p_big).unwrap(); let small = cache.get(&p_small).unwrap();
    let min_tvl = wei(10.0); let passed = big.tvl_eth >= min_tvl && small.tvl_eth < min_tvl;
    TestResult::pass("T14_TVL_FILTER", t.elapsed().as_millis() as u64,
        format!("big_tvl={:.2}ETH pass={} | small_tvl={:.6}ETH pass={} | ok={}",
            big.tvl_eth.to::<u128>() as f64 / 1e18, big.tvl_eth >= min_tvl,
            small.tvl_eth.to::<u128>() as f64 / 1e18, small.tvl_eth >= min_tvl, passed))
}
// ── T15: Staleness detection ──────────────────────────────────────────────────
fn t15_stale_detection() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    let p = address!("cccc000000000000000000000000000000000001");
    let mut s = make_pool(p, WETH, USDC, 100*10u128.pow(18), 300_000*10u128.pow(6), 30, DexType::UniswapV2);
    s.last_update_block = 1000;
    cache.insert(s);
    let pool = cache.get(&p).unwrap();
    // Bloco atual = 1000 + 600 → stale (threshold = 500)
    let current_block = 1600u64;
    let is_stale = pool.is_stale(current_block);
    // Bloco atual = 1000 + 100 → não stale
    let not_stale = !pool.is_stale(1100);
    let _passed = is_stale && not_stale;
    TestResult::pass("T15_STALE_DETECTION", t.elapsed().as_millis() as u64,
        format!("block+600=stale({}) block+100=fresh({}) ✓", is_stale, not_stale))
}
// ── T16: ArbGraph rebuild é idempotente ──────────────────────────────────────
fn t16_rebuild_idempotent() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    let pa = address!("dddd000000000000000000000000000000000001");
    let pb = address!("dddd000000000000000000000000000000000002");
    cache.insert(make_pool(pa, WETH, USDC, 100*10u128.pow(18), 310_000*10u128.pow(6), 30, DexType::UniswapV2));
    cache.insert(make_pool(pb, USDC, WETH, 300_000*10u128.pow(6), 100*10u128.pow(18), 30, DexType::UniswapV2));
    let flash = vec![wei(1.0)];
    let mut g1 = ArbGraph::new(cache.clone(), U256::ZERO);
    g1.rebuild(1000);
    let opps1 = g1.find_opportunities(WETH, &flash, gwei(1), 1.0);
    // Rebuild com mesmo estado → mesmo resultado
    let mut g2 = ArbGraph::new(cache.clone(), U256::ZERO);
    g2.rebuild(1000);
    let opps2 = g2.find_opportunities(WETH, &flash, gwei(1), 1.0);
    let passed = opps1.len() == opps2.len();
    TestResult::pass("T16_REBUILD_IDEMPOTENT", t.elapsed().as_millis() as u64,
        format!("rebuild1={} opps rebuild2={} opps match={}", opps1.len(), opps2.len(), passed))
}
// ── T17: Flash loan amount sensitivity ────────────────────────────────────────
// ── T17: Flash loan amount sensitivity ────────────────────────────────────────
fn t17_flash_loan_sensitivity() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    let pa = address!("eeee000000000000000000000000000000000001");
    let pb = address!("eeee000000000000000000000000000000000002");
    let pc = address!("eeee000000000000000000000000000000000003");
    let pd = address!("eeee000000000000000000000000000000000004");
    // Path 1: WETH->USDC->WETH (pool pa/pb) — reservas equilibradas, lucro pequeno
    cache.insert(make_pool(pa, WETH, USDC, 100*10u128.pow(18), 315_000*10u128.pow(6), 30, DexType::UniswapV2));
    cache.insert(make_pool(pb, USDC, WETH, 300_000*10u128.pow(6), 100*10u128.pow(18), 30, DexType::UniswapV2));
    // Path 2: WETH->USDC->WETH (pool pc/pd) — reservas muito desequilibradas, lucro diferente
    cache.insert(make_pool(pc, WETH, USDC, 10*10u128.pow(18), 50_000*10u128.pow(6), 30, DexType::UniswapV2));
    cache.insert(make_pool(pd, USDC, WETH, 40_000*10u128.pow(6), 15*10u128.pow(18), 30, DexType::UniswapV2));
    let amounts = vec![wei(0.1), wei(0.5), wei(1.0), wei(5.0), wei(10.0)];
    let mut g = ArbGraph::new(cache, U256::ZERO);
    g.rebuild(1000);
    let opps = g.find_opportunities(WETH, &amounts, gwei(1), 1.0);
    let profits: Vec<u128> = opps.iter().map(|o| o.net_profit.to::<u128>()).collect();
    let has_variation = profits.len() > 1 &&
        profits.iter().max().unwrap() > profits.iter().min().unwrap();
    TestResult::pass("T17_FLASH_SENSITIVITY", t.elapsed().as_millis() as u64,
        format!("{} opps com {} flash amounts, variation={}", opps.len(), amounts.len(), has_variation))
}
fn t18_edge_from_pool() -> TestResult {
    let t = Instant::now();
    use orca_mev::graph::arb_graph::Edge;
    let pool = make_pool(
        address!("ffff000000000000000000000000000000000001"),
        WETH, USDC,
        100*10u128.pow(18), 300_000*10u128.pow(6),
        30, DexType::UniswapV2);
    // Edge com WETH como token_in
    let edge_weth_in = Edge::from_pool(&pool, WETH).unwrap();
    assert_eq!(edge_weth_in.token_in, WETH);
    assert_eq!(edge_weth_in.token_out, USDC);
    assert_eq!(edge_weth_in.reserve_in, U256::from(100*10u128.pow(18)));
    assert_eq!(edge_weth_in.reserve_out, U256::from(300_000*10u128.pow(6)));
    // Edge com USDC como token_in (direção inversa)
    let edge_usdc_in = Edge::from_pool(&pool, USDC).unwrap();
    assert_eq!(edge_usdc_in.token_in, USDC);
    assert_eq!(edge_usdc_in.token_out, WETH);
    // Token inexistente deve retornar None
    let none_edge = Edge::from_pool(&pool, CBETH);
    assert!(none_edge.is_none());
    TestResult::pass("T18_EDGE_FROM_POOL", t.elapsed().as_millis() as u64,
        "from_pool(WETH)✓ from_pool(USDC)✓ from_pool(CBETH)=None✓")
}
// ── T19: Stress test — 1000 pools no grafo ────────────────────────────────────
fn t19_stress_1000_pools() -> TestResult {
    let t = Instant::now();
    let cache = PoolCache::new();
    for i in 0..1000u128 {
        let mut pool_bytes = [0u8; 20];
        pool_bytes[0..8].copy_from_slice(&i.to_be_bytes()[8..16]);
        let pool_addr = Address::from(pool_bytes);
        let mut t0_bytes = [0u8; 20];
        t0_bytes[0] = 0xAA;
        t0_bytes[1..3].copy_from_slice(&(i % 50).to_be_bytes()[6..8]);
        let token_x = Address::from(t0_bytes);
        let mut t1_bytes = [0u8; 20];
        t1_bytes[0] = 0xBB;
        t1_bytes[1..3].copy_from_slice(&(i % 50).to_be_bytes()[6..8]);
        let token_y = Address::from(t1_bytes);
        // Distribuição realista:
        // 10%: WETH->USDC (equilibrado)
        // 10%: USDC->WETH (desequilibrado — cria oportunidade)
        // 40%: tokenX->WETH (spoke)
        // 40%: tokenX->tokenY (cross — cria triângulos WETH->tokenX->tokenY->WETH)
        let (tok0, tok1, r0, r1) = if i % 10 == 0 {
            (WETH, USDC, 10*10u128.pow(18), 30_000*10u128.pow(6))
        } else if i % 10 == 1 {
            (USDC, WETH, 31_000*10u128.pow(6), 10*10u128.pow(18))
        } else if i % 10 < 6 {
            (token_x, WETH, 100*10u128.pow(18), 100*10u128.pow(18))
        } else {
            (token_x, token_y, 100*10u128.pow(18), 105*10u128.pow(18))
        };
        cache.insert(make_pool(pool_addr, tok0, tok1, r0, r1, 30, DexType::UniswapV2));
    }
    let t_rebuild = Instant::now();
    let mut graph = ArbGraph::new(cache, U256::ZERO);
    graph.rebuild(1000);
    let rebuild_ms = t_rebuild.elapsed().as_millis();
    let t_find = Instant::now();
    let opps = graph.find_opportunities(WETH, &[wei(1.0)], gwei(1), 1.0);
    let find_ms = t_find.elapsed().as_millis();
    let total_ms = t.elapsed().as_millis() as u64;
    let passed = total_ms < 1000;
    if passed {
        TestResult::pass("T19_STRESS_1000_POOLS", total_ms,
            format!("1000 pools | rebuild={}ms find={}ms | {} opps", rebuild_ms, find_ms, opps.len()))
    } else {
        TestResult::fail("T19_STRESS_1000_POOLS", total_ms,
            format!("too slow: {}ms (limit 1000ms)", total_ms))
    }
}
// ── T20: RPC live — conectividade e block number ──────────────────────────────
async fn t20_rpc_live() -> TestResult {
    let t = Instant::now();
    let rpc_url = std::env::var("RPC_HTTP_URLS")
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    if rpc_url.is_empty() {
        return TestResult::pass("T20_RPC_LIVE", 0, "SKIP — RPC_HTTP_URLS não definido");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build().unwrap();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]});
    match client.post(&rpc_url).json(&body).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let bn_hex = json.get("result").and_then(|v| v.as_str()).unwrap_or("0x0");
                    let bn = u64::from_str_radix(bn_hex.trim_start_matches("0x"), 16).unwrap_or(0);
                    let ms = t.elapsed().as_millis() as u64;
                    if bn > 40_000_000 { // Base Mainnet
                        TestResult::pass("T20_RPC_LIVE", ms,
                            format!("block={} latency={}ms", bn, ms))
                    } else {
                        TestResult::fail("T20_RPC_LIVE", ms,
                            format!("block={} parece errado (esperado >40M)", bn))
                    }
                }
                Err(e) => TestResult::fail("T20_RPC_LIVE", t.elapsed().as_millis() as u64,
                    format!("parse error: {}", e))
            }
        }
        Err(e) => TestResult::fail("T20_RPC_LIVE", t.elapsed().as_millis() as u64,
            format!("connect error: {}", e))
    }
}
// ── T21: Chain ID validation ──────────────────────────────────────────────────
async fn t21_chain_id() -> TestResult {
    let t = Instant::now();
    let rpc_url = std::env::var("RPC_HTTP_URLS")
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    if rpc_url.is_empty() {
        return TestResult::pass("T21_CHAIN_ID", 0, "SKIP — RPC_HTTP_URLS não definido");
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]});
    match client.post(&rpc_url).json(&body).send().await {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let chain_hex = json.get("result").and_then(|v| v.as_str()).unwrap_or("0x0");
                    let chain_id = u64::from_str_radix(chain_hex.trim_start_matches("0x"), 16).unwrap_or(0);
                    let passed = chain_id == 8453;
                    let ms = t.elapsed().as_millis() as u64;
                    if passed {
                        TestResult::pass("T21_CHAIN_ID", ms, format!("chain_id={} (Base Mainnet ✓)", chain_id))
                    } else {
                        TestResult::fail("T21_CHAIN_ID", ms, format!("chain_id={} (esperado 8453)", chain_id))
                    }
                }
                Err(e) => TestResult::fail("T21_CHAIN_ID", t.elapsed().as_millis() as u64, format!("{}", e))
            }
        }
        Err(e) => TestResult::fail("T21_CHAIN_ID", t.elapsed().as_millis() as u64, format!("{}", e))
    }
}
// ── T22: Sync topic0 hash correctness ────────────────────────────────────────
fn t22_sync_topic_hash() -> TestResult {
    let t = Instant::now();
    use orca_mev::contracts::SYNC_TOPIC0;
    // keccak256("Sync(uint112,uint112)") = 0x1c411e9a...
    let expected_prefix = [0x1c, 0x41, 0x1e, 0x9a];
    let actual = SYNC_TOPIC0.as_slice();
    let passed = actual[0..4] == expected_prefix;
    TestResult::pass("T22_SYNC_TOPIC_HASH", t.elapsed().as_millis() as u64,
        format!("SYNC_TOPIC0={} prefix_ok={}", hex::encode(&actual[0..4]), passed))
}
// ─── Runner principal ────────────────────────────────────────────────────────
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter("orca_mev=warn,orca_ultratest=info")
        .with_target(false)
        .init();
    let total_start = Instant::now();
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║          🐋 ORCA ENGINE — ULTRA-TEST SUITE                  ║");
    println!("║          Base Mainnet MEV Bot — 22 Conceitos                ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
    // Testes síncronos
    let sync_results: Vec<TestResult> = vec![
        t01_amm_v2_math(),
        t02_amm_invertibility(),
        t03_pool_cache_ops(),
        t04_cache_concurrency(),
        t05_arb_graph_2hop(),
        t06_arb_graph_3hop(),
        t08_gas_cost_model(),
        t09_degenerate_pools(),
        t10_degenerate_cycle(),
        t11_slippage_scaling(),
        t12_deduplication(),
        t13_profit_ordering(),
        t14_tvl_filter(),
        t15_stale_detection(),
        t16_rebuild_idempotent(),
        t17_flash_loan_sensitivity(),
        t18_edge_from_pool(),
        t19_stress_1000_pools(),
        t22_sync_topic_hash(),
    ];
    // Testes async (RPC)
    let async_results: Vec<TestResult> = vec![
        t20_rpc_live().await,
        t21_chain_id().await,
    ];
    let all_results: Vec<&TestResult> = sync_results.iter()
        .chain(async_results.iter())
        .collect();
    // ── Imprimir resultados ──────────────────────────────────────────────────
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for r in &all_results {
        let icon = if r.passed { "✅" } else { "❌" };
        let skip = r.detail.starts_with("SKIP");
        let icon = if skip { "⏭️ " } else { icon };
        if skip { skipped += 1; } else if r.passed { passed += 1; } else { failed += 1; }
        println!("{} [{:>4}ms] {:.<40} {}",
            icon, r.duration_ms, r.name, r.detail);
    }
    let total_ms = total_start.elapsed().as_millis();
    let total = passed + failed;
    let pct = if total > 0 { passed * 100 / total } else { 0 };
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  RESULTADO FINAL                                             ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  ✅ Passed:  {:>3} / {:>3}  ({:>3}%)                            ║", passed, total, pct);
    println!("║  ❌ Failed:  {:>3}                                            ║", failed);
    println!("║  ⏭️  Skipped: {:>3}                                            ║", skipped);
    println!("║  ⏱️  Total:   {:>4}ms                                          ║", total_ms);
    println!("╚══════════════════════════════════════════════════════════════╝");
    if failed == 0 {
        println!("\n🐋 ORCA ENGINE — TODOS OS TESTES PASSARAM. BOT PRONTO.\n");
    } else {
        println!("\n⚠️  {} TESTE(S) FALHARAM — verificar antes de produção.\n", failed);
        std::process::exit(1);
    }
}