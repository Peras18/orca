//! 🚀 Simple Bootstrap - Inicialização Individual de Reserves
//!
//! Usa pools verificadas na Base e chamadas tipadas via `sol!`.
//! Aerodrome CL / Uni V3 / sAMM não expõem `getReserves()` V2 — são ignorados com log INFO.

use alloy::primitives::{address, Address, U256};
use alloy::providers::Provider;
use alloy::rpc::types::eth::TransactionRequest;
use alloy::sol;
use eyre::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{info, trace};

use crate::cache::PoolCache;
use crate::cache::PoolState;
use crate::contracts::DexType;

sol! {
    #[sol(rpc)]
    interface IUniswapV2Pair {
        function getReserves() external view returns
            (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

/// Pools prioritários (USDC/cbETH, WETH/cbBTC, USDC/USDT vAMM) — dedupe no merge.
const PRIORITY_BOOTSTRAP: [(Address, DexType); 3] = [
    (
        address!("0xa1d2ee6212d4375f154393e59895a5d27859c4e4"),
        DexType::Aerodrome,
    ), // USDC / cbETH — Aerodrome vAMM (GeckoTerminal)
    (
        address!("0x2578365b3dfa7ffe60108e181efb79feddec2319"),
        DexType::Aerodrome,
    ), // WETH / cbBTC — Aerodrome vAMM
    (
        address!("0x96508ae8037c6bd16162620187691f1c1e3e07c1"),
        DexType::Aerodrome,
    ), // USDC / USDT — Aerodrome vAMM
];

/// Lista principal (20 pools V2-compatible verificadas via `getReserves` na Base).
/// Prioridades repetidas no topo para garantir ordem; `merged_bootstrap_targets` dedupe.
const MAIN_BOOTSTRAP_POOLS: [(Address, DexType); 43] = [
    (
        address!("0xa1d2ee6212d4375f154393e59895a5d27859c4e4"),
        DexType::Aerodrome,
    ),
    (
        address!("0x2578365b3dfa7ffe60108e181efb79feddec2319"),
        DexType::Aerodrome,
    ),
    (
        address!("0x96508ae8037c6bd16162620187691f1c1e3e07c1"),
        DexType::Aerodrome,
    ),
    (
        address!("0x6cDcb1C4A4D1C3C6d054b27AC5B77e89eAFb971d"),
        DexType::Aerodrome,
    ),
    (
        address!("0x67b00B46FA4f4F24c03855c5C8013C0B938B3eEc"),
        DexType::Aerodrome,
    ),
    (
        address!("0xcDAC0d6c6C59727a65F871236188350531885C43"),
        DexType::UniswapV2,
    ), // BaseSwap cbETH/WETH
    (
        address!("0x88A43bbDF9D098eEC7bCEda4e2494615dfD9bB9C"),
        DexType::UniswapV2,
    ), // BaseSwap WETH/USDC
    (
        address!("0x6b2a379f803923542047ac2c7f268ffe7989d869"),
        DexType::UniswapV2,
    ), // Uni V2 USDC/USDT
    (
        address!("0x7f670f78b17dec44d5ef68a48740b6f8849cc2e6"),
        DexType::Aerodrome,
    ),
    (
        address!("0x8b49c7ec53cb4ca3666bb16727fc5c5f6d12226f"),
        DexType::Aerodrome,
    ),
    (
        address!("0xb0d931138bf96501654f2268cdd84420151ff52e"),
        DexType::Aerodrome,
    ),
    (
        address!("0x89d0f320ac73dd7d9513ffc5bc58d1161452a657"),
        DexType::Aerodrome,
    ),
    (
        address!("0x4910c78a1c75e36548f1f2f1f4dcb71a4f69fc07"),
        DexType::Aerodrome,
    ),
    (
        address!("0x01784ef301d79e4b2df3a21ad9a536d4cf09a5ce"),
        DexType::Aerodrome,
    ),
    (
        address!("0xae37030302643ee121dfa50e76b27b90fac9e872"),
        DexType::Aerodrome,
    ),
    (
        address!("0xf65d8d39f5e2b85ccd34e6a74f09cea922d3ede1"),
        DexType::Aerodrome,
    ),
    (
        address!("0x42781ec558f9fb95f5e080572bcd0a37523b55e2"),
        DexType::Aerodrome,
    ),
    (
        address!("0x9f82fc01ac38dc8f85b1d59614b0c03cae9e19b7"),
        DexType::Aerodrome,
    ),
    (
        address!("0x8966379fcd16f7cb6c6ea61077b6c4fafeca28f4"),
        DexType::Aerodrome,
    ),
    (
        address!("0x66e1031db440890a824f0db80489be979bd16a86"),
        DexType::Aerodrome,
    ),
    (address!("0xc4838fbeef72ba0719a4c4f1efeec68991b74a20"), DexType::UniswapV3),
    (address!("0x9187c24a3a81618f07a9722b935617458f532737"), DexType::UniswapV3),
    (address!("0xf39b8ad8b194a56291d55b3a4c690b2557b5b8c9"), DexType::Aerodrome), // USDC/0xa69f80
    (address!("0x7ec6c9d993d9832aa654593f2dbc21303650bc6c"), DexType::Aerodrome),
    (address!("0xa213a86c7f279ee13e0b45642483a00f917821c2"), DexType::Aerodrome),
    (address!("0xa135b59fe221c0c8d441294f97f96fbc37bc9fbe"), DexType::Aerodrome),
    (address!("0x0ab02e160f0df68dc049b012c514857306960eae"), DexType::Aerodrome),
    (address!("0x26e54b556c6ec78bc6d8b9ca9c82ed8c548ccba6"), DexType::Aerodrome),
    (address!("0x782d7c494d5ddc20c246a82ac8fe277e2728d002"), DexType::Aerodrome),
    (address!("0x3f413fccaea59b8053d605aea7ae847c02ed5d95"), DexType::Aerodrome),
    (address!("0x659be70647b0f63217d60e077f4417b1ecc65064"), DexType::Aerodrome),
    (address!("0x01271a205e3f37cbfc9b353170d726060e193c0d"), DexType::Aerodrome),
    (address!("0xee8690f5c25146ee3f163b399099a3e28e59b6a3"), DexType::Aerodrome),
    (address!("0xbe4c36b9542610df83ca690c8b5bc53bbbc5d542"), DexType::Aerodrome),
    (address!("0x7f1a5b66ba3bb56c4b68cfc353a5e041c9763a4c"), DexType::Aerodrome),
    (address!("0x2400e1e764556e19e28bc9e3c685e4104bf152f8"), DexType::Aerodrome),
    (address!("0x0392b12a1ceb0cd13af5ea448cf5586ea609852d"), DexType::Aerodrome),
    (address!("0xf4d97f2da56e8c3098f3a8d538db630a2606a024"), DexType::Aerodrome),
    (address!("0x46d398a5b33709877f50c8918a7ee96f1be1d7dd"), DexType::Aerodrome),
    (address!("0x9520e1a3bfd86da6c1e9e5ee4b9c2f11c413358f"), DexType::Aerodrome),
    (address!("0x6b0f53cbd9272d8117e9535fe25371dedf39a1be"), DexType::Aerodrome),
    (address!("0x3693022bd390e147d8dd89a05403c80ff21dd64b"), DexType::Aerodrome),
    (address!("0x443d60c2f5cc88a955bee631fc7fad08df7db3a0"), DexType::Aerodrome),
];

/// Pools Uniswap V3 para bootstrap (WETH/USDC com diferentes fees)
const V3_BOOTSTRAP_POOLS: [(Address, u32, u32); 5] = [
    (
        address!("0xb2cc224c1c9fee385f8ad6a55b4d94e92359dc59"),
        100,
        18,
    ), // WETH/USDC fee=100
    (
        address!("0xd0b53d9277642d899df5c87a3966a349a798f224"),
        500,
        18,
    ), // WETH/USDC fee=500
    (
        address!("0x80cc08712aa61ce9dc7604f9ce7560a25094b862"),
        10000,
        18,
    ), // DEGEN/WETH fee=1% — volume enorme
    (
        address!("0x682a02d5a32ddb09d1cf4791fb4124e0d4e17b67"),
        3000,
        18,
    ), // BRETT/WETH fee=0.3% — volume alto
    (
        address!("0x4ff9fb8d73d65f4c255a4d3ebf5f957969fef2e2"),
        3000,
        18,
    ), // AERO/WETH fee=0.3%
];
const WETH_BASE: Address = address!("0x4200000000000000000000000000000000000006");
const USDC_BASE: Address = address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

const BOOTSTRAP_VERIFY_TARGET: usize = 500;

fn merged_bootstrap_targets(pool_cache: &PoolCache) -> Vec<(Address, DexType)> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(BOOTSTRAP_VERIFY_TARGET);
    for &(addr, dex) in PRIORITY_BOOTSTRAP.iter() {
        if seen.insert(addr) { out.push((addr, dex)); }
    }
    for &(addr, dex) in MAIN_BOOTSTRAP_POOLS.iter() {
        if out.len() >= BOOTSTRAP_VERIFY_TARGET { break; }
        if seen.insert(addr) { out.push((addr, dex)); }
    }
    // Adiciona pools do cache persistente (ON-THE-FLY + discovery)
    for state in pool_cache.get_sample_pools(pool_cache.len()) {
        if out.len() >= BOOTSTRAP_VERIFY_TARGET { break; }
        if seen.insert(state.address) {
            out.push((state.address, DexType::UniswapV2));
        }
    }
    out
}

fn bootstrap_skip_reason(dex: DexType) -> Option<&'static str> {
    match dex {
        DexType::UniswapV2 | DexType::Aerodrome => None,
        DexType::UniswapV3 | DexType::PancakeSwap => {
            // V3 agora é bootstrapado via slot0+liquidity em separado
            Some("pool V3 — bootstrapado via slot0/liquidity em separado")
        }
        DexType::AerodromeStable => {
            Some("pool Aerodrome sAMM — interface estável; getReserves V2 skipped")
        }
    }
}

/// Bootstrap simplificado — sem Multicall3
pub async fn bootstrap_simple<P>(
    provider: Arc<P>,
    pool_cache: Arc<PoolCache>,
    _pool_addresses: &[Address],
) -> Result<usize>
where
    P: Provider + Clone + 'static,
{
    let targets = merged_bootstrap_targets(&pool_cache);

    info!("🚀 [BOOTSTRAP SIMPLE] Inicializando reserves individualmente...");
    info!(
        "🚀 [BOOTSTRAP SIMPLE] Target: {} pools verificadas (max {})",
        targets.len(),
        BOOTSTRAP_VERIFY_TARGET
    );

    // Garantir pools prioritárias registadas no cache antes das chamadas RPC
    for &(addr, dex) in PRIORITY_BOOTSTRAP.iter() {
        if !pool_cache.contains(&addr) {
            pool_cache.insert(PoolState::new(
                addr,
                Address::ZERO,
                Address::ZERO,
                3000,
                dex,
            ));
            info!(
                "[BOOTSTRAP] Pool prioritária adicionada ao cache (placeholder): {:?}",
                addr
            );
        }
    }

    let current_block = provider.get_block_number().await.unwrap_or(0);
    let mut initialized = 0usize;

    for (pool_addr, dex) in targets.iter().copied() {
        if let Some(reason) = bootstrap_skip_reason(dex) {
            info!(
                "[BOOTSTRAP] Pool {:?} dex {:?}: {}",
                pool_addr, dex, reason
            );
            continue;
        }

        let contract = IUniswapV2Pair::new(pool_addr, provider.clone());
        match contract.getReserves().call().await {
            Ok(result) => {
                let r0 = U256::from(result.reserve0);
                let r1 = U256::from(result.reserve1);
                if r0 > U256::ZERO && r1 > U256::ZERO {
                    let (t0, t1) = match (contract.token0().call().await, contract.token1().call().await)
                    {
                        (Ok(t0), Ok(t1)) => (t0._0, t1._0),
                        _ => (Address::ZERO, Address::ZERO),
                    };

                    let mut state = pool_cache.get(&pool_addr).unwrap_or_else(|| {
                        PoolState::new(pool_addr, Address::ZERO, Address::ZERO, 3000, dex)
                    });
                    state.dex_type = dex;
                    if state.token0 == Address::ZERO && t0 != Address::ZERO {
                        state.token0 = t0;
                    }
                    if state.token1 == Address::ZERO && t1 != Address::ZERO {
                        state.token1 = t1;
                    }
                    if state.token0 != Address::ZERO && state.token1 != Address::ZERO { state.reserve_verified = true; pool_cache.insert(state); }
                    pool_cache.update_tokens(pool_addr, t0, t1);

                    pool_cache.update_sync_event(pool_addr, r0, r1, current_block);
                    info!(
                        "[BOOTSTRAP] ✅ Pool {:?}: r0={} r1={} t0={:?} t1={:?} dex={:?}",
                        pool_addr, r0, r1, t0, t1, dex
                    );
                    initialized += 1;
                } else {
                    trace!(
                        "[BOOTSTRAP] Pool {:?} reserves zero após getReserves — skip silencioso",
                        pool_addr
                    );
                }
            }
            Err(_) => {
                // Caminho “desconhecido” / contrato incompatível: não WARN (evita ruído tipo buffer overrun)
                trace!(
                    "[BOOTSTRAP] getReserves falhou para {:?} dex {:?} — skip silencioso",
                    pool_addr,
                    dex
                );
            }
        }
    }

    // ── Bootstrap V3 pools via slot0() + liquidity() ──
    let q96 = U256::from(1u128) << 96;
    for &(pool_addr, fee_ppm, _decimals) in V3_BOOTSTRAP_POOLS.iter() {
        // A) slot0() — selector 0x3850c7bd
        let slot0_call = TransactionRequest::default()
            .to(pool_addr)
            .input(vec![0x38, 0x50, 0xc7, 0xbd].into());
        let sqrt_price_x96 = match provider.call(&slot0_call).await {
            Ok(data) if data.len() >= 32 => {
                let sp = U256::from_be_slice(&data[0..32]);
                info!("[BOOTSTRAP V3] slot0 {:?} sqrtPriceX96={}", pool_addr, sp);
                sp
            }
            Ok(_) => {
                trace!("[BOOTSTRAP V3] slot0 resposta curta para {:?}", pool_addr);
                continue;
            }
            Err(e) => {
                trace!("[BOOTSTRAP V3] slot0 falhou para {:?}: {}", pool_addr, e);
                continue;
            }
        };

        // B) liquidity() — selector 0x1a686502
        let liquidity_call = TransactionRequest::default()
            .to(pool_addr)
            .input(vec![0x1a, 0x68, 0x65, 0x02].into());
        let liquidity = match provider.call(&liquidity_call).await {
            Ok(data) if data.len() >= 32 => {
                // uint128 right-aligned in 32-byte slot
                let liq = u128::from_be_bytes(data[16..32].try_into().unwrap_or([0u8; 16]));
                info!("[BOOTSTRAP V3] liquidity {:?} L={}", pool_addr, liq);
                liq
            }
            Ok(_) => {
                trace!("[BOOTSTRAP V3] liquidity resposta curta para {:?}", pool_addr);
                continue;
            }
            Err(e) => {
                trace!("[BOOTSTRAP V3] liquidity falhou para {:?}: {}", pool_addr, e);
                continue;
            }
        };

        // Converter para reserves virtuais (tudo U256, sem f64)
        let liq_u256 = U256::from(liquidity);
        let reserve0 = liq_u256
            .checked_mul(q96)
            .and_then(|v| v.checked_div(sqrt_price_x96))
            .unwrap_or(U256::ZERO);
        let reserve1 = liq_u256
            .checked_mul(sqrt_price_x96)
            .and_then(|v| v.checked_div(q96))
            .unwrap_or(U256::ZERO);

        // Validação
        if sqrt_price_x96.is_zero() {
            info!("[BOOTSTRAP V3] {:?} skip: sqrtPriceX96 = 0 (não inicializado)", pool_addr);
            continue;
        }
        if liquidity == 0 {
            info!("[BOOTSTRAP V3] {:?} skip: liquidez = 0", pool_addr);
            continue;
        }
        if reserve1.is_zero() {
            info!("[BOOTSTRAP V3] {:?} skip: reserve1 = 0", pool_addr);
            continue;
        }

        // Descobrir tokens reais via token0()/token1()
        let token0_call = TransactionRequest::default()
            .to(pool_addr)
            .input(vec![0x0d, 0xfe, 0x16, 0x81].into()); // token0() selector
        let token1_call = TransactionRequest::default()
            .to(pool_addr)
            .input(vec![0xd2, 0x12, 0x20, 0xa7].into()); // token1() selector

        let t0 = match provider.call(&token0_call).await {
            Ok(data) if data.len() >= 32 => Address::from_slice(&data[12..32]),
            _ => WETH_BASE, // fallback
        };
        let t1 = match provider.call(&token1_call).await {
            Ok(data) if data.len() >= 32 => Address::from_slice(&data[12..32]),
            _ => USDC_BASE, // fallback
        };

        // decimals: assumir 18 para token0, 6 se token1 é USDC, 18 caso contrário
        let dec1 = if t1 == USDC_BASE { 6u8 } else { 18u8 };

        let mut state = PoolState::new(pool_addr, t0, t1, fee_ppm, DexType::UniswapV3);
        state.reserve0 = reserve0;
        state.reserve1 = reserve1;
        state.decimals0 = 18;
        state.decimals1 = dec1;
        state.sqrt_price_x96 = Some(sqrt_price_x96.try_into().unwrap_or(u128::MAX));
        state.liquidity = Some(liquidity);
        state.last_update_block = current_block;
        state.reserve_verified = true;
        pool_cache.insert(state);
        pool_cache.update_tokens(pool_addr, t0, t1);
        info!(
            "[BOOTSTRAP] ✅ V3 Pool {:?}: fee={} r0={} r1={} t0={:?} t1={:?} | sqrtPriceX96={} liquidity={}",
            pool_addr, fee_ppm, reserve0, reserve1, t0, t1, sqrt_price_x96, liquidity
        );
        initialized += 1;
    }

    info!(
        "[BOOTSTRAP] {}/{} pools inicializadas (lista verificada + V3)",
        initialized,
        targets.len().min(BOOTSTRAP_VERIFY_TARGET) + V3_BOOTSTRAP_POOLS.len()
    );

    let pools_with_reserves = pool_cache.get_sample_pools(pool_cache.len());
    let pools_count = pools_with_reserves
        .iter()
        .filter(|p| !p.reserve0.is_zero() && !p.reserve1.is_zero())
        .count();

    let mut token_counts: HashMap<Address, usize> = HashMap::new();
    for p in pools_with_reserves
        .iter()
        .filter(|p| !p.reserve0.is_zero() && !p.reserve1.is_zero())
    {
        *token_counts.entry(p.token0).or_insert(0) += 1;
        *token_counts.entry(p.token1).or_insert(0) += 1;
    }
    let unique_tokens = token_counts.len();

    let estimated_cycles: usize = token_counts
        .values()
        .map(|&n| if n >= 3 { (n * (n - 1) * (n - 2)) / 6 } else { 0 })
        .sum();

    info!(
        "[GRAPH] Pools com reserves: {} | Tokens únicos: {} | Possíveis ciclos: {}",
        pools_count, unique_tokens, estimated_cycles
    );

    Ok(initialized)
}


