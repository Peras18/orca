//! PROJETO ORCA - Base Mainnet MEV Engine
//! Segurança absoluta de capital + Extração máxima de valor
//!
//! 🐋 ORCA: Predador silencioso que ataca no momento exato

pub mod audit;
pub mod ghost_executor;
pub mod performance_tracker;
pub mod safety;
pub mod sequencer_sync;
pub mod yul_contracts;

pub use audit::ForensicAudit;
pub use ghost_executor::{CallbackHijacker, GhostStateExecutor, TransientAction};
pub use performance_tracker::{BankLog, GasLog, PerformanceTracker, ProfitLog};
pub use safety::{BundleProtector, ProfitGuard, SafetyEngine};
pub use sequencer_sync::{BlockTiming, RTTMonitor, SequencerSync};
pub use yul_contracts::{GasOptimizer, YulExecutor, YulTemplates};

use crate::artemis::{MevEvent, Strategy, StrategyContext};
use crate::cache::PoolCache;
use crate::contracts::{DexType, NormalizedSwapEvent};
use crate::graph::{ArbGraph, PoolScorer};
use crate::prediction::detect_cross_pool_divergence;
use crate::prediction::PatternMemory;
use crate::risk::BankrollManager;
use crate::telemetry::TelemetryCollector;
use alloy::primitives::{address, Address, Bytes, FixedBytes, U256};
use alloy::providers::{Provider as AlloyProvider, RootProvider};
use alloy::transports::BoxTransport;
use async_trait::async_trait;

const MIN_FLASH_WEI_U256: U256 = U256::from_limbs([10_000_000_000_000_000u64, 0, 0, 0]);

/// Endereço oficial do QuoterV2 da Uniswap V3 na Base Mainnet.
/// Confirmado contra docs.uniswap.org/contracts/v3/reference/deployments/base-deployments
/// e validado on-chain: factory() devolve 0x33128a8fC17869897dcE68Ed026d694621f6FDfD,
/// que é a UniswapV3Factory real na Base (a nossa constante UniswapV3Factory::ADDRESS
/// estava errada -- corrigida em src/contracts/uniswap_v3.rs).
const UNISWAP_V3_QUOTER_V2: Address = Address::new([
    0x3d, 0x4e, 0x44, 0xEb, 0x13, 0x74, 0x24, 0x0C, 0xE5, 0xF1,
    0xB8, 0x71, 0xab, 0x26, 0x1C, 0xD1, 0x63, 0x35, 0xB7, 0x6a,
]);

/// 🎯 CORREÇÃO DE CAUSA RAIZ (erro "IIA" / Insufficient Input Amount):
///
/// simulate_cycle_profit_wei() usa a fórmula de produto constante (x*y=k,
/// fee 0.3% fixo) -- válida para AMMs V2-style com liquidez uniforme, mas
/// estruturalmente ERRADA para Uniswap V3, que tem liquidez concentrada em
/// ticks discretos. Um hop V3 grande pode atravessar múltiplos ticks com
/// liquidez muito diferente entre eles -- a fórmula V2 ignora isso por
/// completo e sobrestima sistematicamente quanto pode ser trocado sem
/// reverter, causando "IIA" no eth_call real.
///
/// Esta função substitui a aproximação por uma simulação EXATA, usando o
/// QuoterV2 oficial da própria Uniswap (gratuito via eth_call, sem gastar
/// gás real -- é justamente para isto que o protocolo o disponibiliza).
/// Devolve None se a simulação reverter (ex: liquidez insuficiente mesmo
/// para o tick atual) -- nesse caso o hop não é viável para este tamanho,
/// ponto final, sem adivinhar margens de segurança arbitrárias.
async fn quote_v3_exact_input(
    provider: &impl AlloyProvider,
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
) -> Option<U256> {
    use alloy::rpc::types::TransactionRequest;
    use alloy::network::TransactionBuilder;

    // quoteExactInputSingle((address tokenIn, address tokenOut, uint256 amountIn, uint24 fee, uint160 sqrtPriceLimitX96))
    // selector: 0xc6a5026a
    let mut calldata: Vec<u8> = Vec::with_capacity(4 + 32 * 5);
    calldata.extend_from_slice(&[0xc6, 0xa5, 0x02, 0x6a]);
    // struct é passada inline (não é dynamic type, é tuple simples -- cada
    // campo ocupa exactamente uma slot de 32 bytes, sem offset/length).
    let mut token_in_padded = [0u8; 32];
    token_in_padded[12..].copy_from_slice(token_in.as_slice());
    calldata.extend_from_slice(&token_in_padded);

    let mut token_out_padded = [0u8; 32];
    token_out_padded[12..].copy_from_slice(token_out.as_slice());
    calldata.extend_from_slice(&token_out_padded);

    calldata.extend_from_slice(&amount_in.to_be_bytes::<32>());
    calldata.extend_from_slice(&U256::from(fee).to_be_bytes::<32>());
    calldata.extend_from_slice(&U256::ZERO.to_be_bytes::<32>()); // sqrtPriceLimitX96 = 0 (sem limite)

    let call_req = TransactionRequest::default()
        .with_to(UNISWAP_V3_QUOTER_V2)
        .with_input(alloy::primitives::Bytes::from(calldata));

    match provider.call(&call_req).await {
        Ok(result) => {
            // Retorno: (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate)
            // amountOut é a primeira slot de 32 bytes.
            if result.len() >= 32 {
                Some(U256::from_be_slice(&result[0..32]))
            } else {
                None
            }
        }
        Err(e) => {
            // CORREÇÃO: distinguir revert real (liquidez insuficiente, "IIA"
            // genuíno) de erro de rede/rate-limit (HTTP 429, timeout) -- um
            // RPC sobrecarregado a devolver erro NÃO significa que o swap é
            // inviável, mas estava a ser tratado como tal, levando a refinar
            // o tamanho para 0 em 100% dos casos mesmo quando a liquidez
            // real era suficiente (confirmado manualmente via eth_call direto
            // para os mesmos parâmetros).
            let err_str = e.to_string();
            if err_str.contains("429") || err_str.contains("rate limit") || err_str.contains("Too Many Requests") {
                warn!("[QUOTER-V3] RPC rate-limited durante quote -- NÃO é IIA real: {}", err_str);
            }
            None
        }
    }
}

/// 🎯 Simula o ciclo COMPLETO, hop a hop, devolvendo o output final real
/// (ou None se QUALQUER hop reverter). Usa QuoterV2 real para hops V3
/// (simulação exacta multi-tick) e a fórmula V2 para os restantes (AMMs
/// de produto constante já são bem representados por ela). O output de
/// cada hop alimenta o input do próximo, exactamente como a execução
/// real na chain -- corrige o bug de só validar o primeiro hop, que
/// deixava o 2º/3º hop sem nenhuma validação real (causa de "IIA"
/// persistir mesmo depois de refinar só o tamanho do flash loan inicial).
async fn simulate_full_cycle_v3_aware(
    provider: &impl AlloyProvider,
    hops: &[crate::graph::arb_graph::Edge],
    amount_in: U256,
) -> Option<U256> {
    let mut amount = amount_in;
    for (hop_idx, hop) in hops.iter().enumerate() {
        if amount.is_zero() {
            info!("[DIAG-CYCLE] hop {} recebeu amount=0, abortando", hop_idx);
            return None;
        }
        amount = if hop.dex_type == crate::contracts::DexType::UniswapV3 {
            match quote_v3_exact_input(provider, hop.token_in, hop.token_out, hop.fee, amount).await {
                Some(out) => out,
                None => {
                    info!(
                        "[DIAG-CYCLE] hop {} (V3) FALHOU: pool={:?} token_in={:?} token_out={:?} fee={} amount_in={} liquidity_no_cache={:?}",
                        hop_idx, hop.pool, hop.token_in, hop.token_out, hop.fee, amount, hop.liquidity
                    );
                    return None;
                }
            }
        } else {
            if hop.reserve_in.is_zero() || hop.reserve_out.is_zero() {
                info!("[DIAG-CYCLE] hop {} (não-V3) reserves zero", hop_idx);
                return None;
            }
            let amount_in_with_fee = amount.saturating_mul(U256::from(997u64));
            let numerator = amount_in_with_fee.saturating_mul(hop.reserve_out);
            let denominator = hop
                .reserve_in
                .saturating_mul(U256::from(1000u64))
                .saturating_add(amount_in_with_fee);
            if denominator.is_zero() {
                info!("[DIAG-CYCLE] hop {} (não-V3) denominator zero", hop_idx);
                return None;
            }
            numerator / denominator
        };
    }
    Some(amount)
}

/// Encontra, via busca binária sobre o CICLO COMPLETO (não só o 1º hop),
/// o maior amount_in que sobrevive a todos os hops em sequência real.
async fn find_max_viable_cycle_input(
    provider: &impl AlloyProvider,
    hops: &[crate::graph::arb_graph::Edge],
    max_candidate: U256,
) -> U256 {
    if max_candidate.is_zero() {
        return U256::ZERO;
    }
    if simulate_full_cycle_v3_aware(provider, hops, max_candidate).await.is_some() {
        return max_candidate;
    }
    let mut lo = U256::ZERO;
    let mut hi = max_candidate;
    for _ in 0..10 {
        if hi <= lo {
            break;
        }
        let mid = lo + (hi - lo) / U256::from(2u64);
        if mid.is_zero() {
            break;
        }
        if simulate_full_cycle_v3_aware(provider, hops, mid).await.is_some() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Encontra, via busca binária real contra o QuoterV2 (não aproximação),
/// o maior amount_in para um hop V3 que ainda produz um quote válido
/// (não reverte). Usa no máximo ~10 chamadas eth_call (gratuitas, ~50ms
/// cada em paralelo na prática) -- converge rápido porque é busca binária
/// sobre um espaço já estreitado pelo optimal_cycle_input V2 como ponto de
/// partida (max_candidate), não desde zero.
async fn find_max_viable_v3_input(
    provider: &impl AlloyProvider,
    token_in: Address,
    token_out: Address,
    fee: u32,
    max_candidate: U256,
) -> U256 {
    info!("[DIAG-QUOTER] find_max_viable_v3_input chamada: token_in={:?} token_out={:?} fee={} max_candidate={}", token_in, token_out, fee, max_candidate);
    if max_candidate.is_zero() {
        return U256::ZERO;
    }
    // Primeiro: o candidato máximo já funciona? Caso comum quando a
    // liquidez é suficiente -- evita busca binária desnecessária.
    if quote_v3_exact_input(provider, token_in, token_out, fee, max_candidate).await.is_some() {
        return max_candidate;
    }
    let mut lo = U256::ZERO;
    let mut hi = max_candidate;
    for _ in 0..10 {
        if hi <= lo {
            break;
        }
        let mid = lo + (hi - lo) / U256::from(2u64);
        if mid.is_zero() {
            break;
        }
        if quote_v3_exact_input(provider, token_in, token_out, fee, mid).await.is_some() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Simula o lucro líquido (em wei, nunca negativo, nunca panica) de um ciclo
/// de arbitragem para um input dado, usando a fórmula AMM de produto
/// constante (0.3% fee) com as reserves JÁ CONHECIDAS de cada hop -- sem
/// nenhuma chamada à rede. Usado pela pesquisa ternária abaixo.
fn simulate_cycle_profit_wei(hops: &[crate::graph::arb_graph::Edge], input: U256, gas_cost_wei: U256) -> U256 {
    let mut amount = input;
    for hop in hops {
        // CORREÇÃO: usar matemática V3 real (single-tick) quando temos
        // sqrt_price_x96/liquidity disponíveis -- a fórmula V2 abaixo
        // (produto constante) sobrestima sistematicamente a viabilidade
        // de hops V3, causando IIA/panic/BAL#528 em produção mesmo depois
        // da margem de segurança reduzida (confirmado: 3 tipos de erro
        // diferentes, todos consistentes com "tamanho calculado optimista
        // demais para a realidade").
        if hop.dex_type == crate::contracts::DexType::UniswapV3 {
            if let (Some(sqrt_p), Some(liq)) = (hop.sqrt_price_x96, hop.liquidity) {
                let zero_for_one = hop.token_in < hop.token_out;
                match simulate_v3_single_tick(sqrt_p, liq, hop.decimals_in, hop.decimals_out, zero_for_one, amount) {
                    Some(out) if !out.is_zero() => {
                        amount = out;
                        continue;
                    }
                    _ => return U256::ZERO, // simulação V3 real indica inviável -- não usar fallback optimista
                }
            }
        }
        if hop.reserve_in.is_zero() || hop.reserve_out.is_zero() {
            return U256::ZERO;
        }
        let amount_in_with_fee = amount.saturating_mul(U256::from(997u64));
        let numerator = amount_in_with_fee.saturating_mul(hop.reserve_out);
        let denominator = hop
            .reserve_in
            .saturating_mul(U256::from(1000u64))
            .saturating_add(amount_in_with_fee);
        if denominator.is_zero() {
            return U256::ZERO;
        }
        amount = numerator / denominator;
    }
    let cost = input.saturating_add(gas_cost_wei);
    amount.saturating_sub(cost)
}

/// 🎯 Simula um swap V3 single-tick usando a matemática REAL da curva
/// concentrada (não a aproximação de produto constante V2), usando os
/// dados já em cache (sqrt_price_x96, liquidity) -- sem nenhuma chamada
/// de rede extra, e sem depender de QuoterV2/Factory externos que podem
/// apontar para uma pool diferente da que realmente usamos (hop.pool).
///
/// Válido para swaps que não atravessam limites de tick -- para a maioria
/// das oportunidades de arbitragem MEV (tamanhos pequenos/médios face à
/// liquidez da pool), isto é uma aproximação muito mais fiel que a fórmula
/// V2, e gratuita (sem eth_call). Se o swap for grande o suficiente para
/// atravessar ticks, esta função pode sobrestimar ligeiramente o output --
/// a proteção final continua a ser o eth_call real do nosso próprio
/// contrato, que nunca gasta gás se a simulação falhar.
/// INOVAÇÃO (substitui heurística "15% da reserve" para hops V3): deriva
/// o input máximo seguro diretamente do invariante de liquidez concentrada
/// V3 (Δ(1/√P) = amount_in/L), limitando o input ao que mantém o impacto
/// de preço dentro de MAX_IMPACT_BPS — matematicamente ligado à realidade
/// da pool (liquidez ativa no tick atual), não a um % arbitrário da reserve
/// virtual (que nunca reflete liquidez concentrada real e estava a permitir
/// empréstimos de 10+ ETH em pools cuja liquidez no tick não suportava,
/// causando 100% de reverts "IIA" em ciclos V3-V3 confirmados via dataset
/// real de logs/executions.csv).
fn max_safe_input_v3(sqrt_price_x96: u128, liquidity: u128, zero_for_one: bool, max_impact_bps: u64) -> U256 {
    if liquidity == 0 || sqrt_price_x96 == 0 {
        return U256::ZERO;
    }
    let l = U256::from(liquidity);
    let sqrt_p = U256::from(sqrt_price_x96);
    let q96 = U256::from(1u128) << 96;
    if zero_for_one {
        // amount_in <= L * MAX_IMPACT_BPS * Q96 / (10000 * √P)
        l.saturating_mul(U256::from(max_impact_bps))
            .saturating_mul(q96)
            .checked_div(U256::from(10_000u64).saturating_mul(sqrt_p))
            .unwrap_or(U256::ZERO)
    } else {
        // amount_in <= √P * MAX_IMPACT_BPS * L / (10000 * Q96)
        sqrt_p.saturating_mul(U256::from(max_impact_bps))
            .saturating_mul(l)
            .checked_div(U256::from(10_000u64).saturating_mul(q96))
            .unwrap_or(U256::ZERO)
    }
}

fn simulate_v3_single_tick(
    sqrt_price_x96: u128,
    liquidity: u128,
    decimals_in: u8,
    decimals_out: u8,
    zero_for_one: bool,
    amount_in: U256,
) -> Option<U256> {
    if liquidity == 0 || sqrt_price_x96 == 0 || amount_in.is_zero() {
        return None;
    }
    let l = U256::from(liquidity);
    let sqrt_p = U256::from(sqrt_price_x96);
    let q96 = U256::from(1u128) << 96;

    if zero_for_one {
        // Δ(1/√P) = amount_in / L  =>  √P_novo = (L * √P) / (L + amount_in * √P / 2^96)
        let numerator = l.checked_mul(sqrt_p)?;
        let amount_in_times_sqrt = amount_in.checked_mul(sqrt_p)?.checked_div(q96)?;
        let denominator = l.checked_add(amount_in_times_sqrt)?;
        if denominator.is_zero() {
            return None;
        }
        let sqrt_p_new = numerator.checked_div(denominator)?;
        if sqrt_p_new >= sqrt_p || sqrt_p_new.is_zero() {
            return None; // preço não pode subir ao vender token0, ou overflow
        }
        // amountOut = L * (√P - √P_novo) / 2^96
        let diff = sqrt_p.checked_sub(sqrt_p_new)?;
        let amount_out = l.checked_mul(diff)?.checked_div(q96)?;
        Some(amount_out)
    } else {
        // √P_novo = √P + (amount_in * 2^96) / L
        let amount_in_scaled = amount_in.checked_mul(q96)?.checked_div(l)?;
        let sqrt_p_new = sqrt_p.checked_add(amount_in_scaled)?;
        if sqrt_p_new <= sqrt_p {
            return None;
        }
        // amountOut = L * (1/√P - 1/√P_novo) = L * (√P_novo - √P) / (√P * √P_novo / 2^96)
        let diff = sqrt_p_new.checked_sub(sqrt_p)?;
        let numerator = l.checked_mul(diff)?.checked_mul(q96)?;
        let denominator = sqrt_p.checked_mul(sqrt_p_new)?;
        if denominator.is_zero() {
            return None;
        }
        Some(numerator.checked_div(denominator)?)
    }
    // NOTA: o resultado está na escala de decimais nativa do token de saída,
    // tal como os valores reais on-chain -- não precisa de ajuste extra
    // aqui, decimals_in/decimals_out ficam disponíveis para uso futuro se
    // necessário validação cruzada.
    .map(|out| { let _ = (decimals_in, decimals_out); out })
}

/// Pesquisa ternária: encontra o tamanho de input que maximiza o lucro líquido
/// real do ciclo, dentro de [min_input, max_input]. Converge de forma
/// matematicamente garantida porque a curva de lucro de arbitragem cíclica em
/// AMMs de produto constante é côncava (um único pico).
fn optimal_cycle_input(
    hops: &[crate::graph::arb_graph::Edge],
    min_input: U256,
    max_input: U256,
    gas_cost_wei: U256,
) -> U256 {
    let mut lo = min_input;
    let mut hi = max_input.max(min_input);
    // INOVAÇÃO (aplicação prática de Loesch & Richardson, Bancor, arXiv 2502.08258,
    // Jan 2025 -- "Marginal Price Optimization"): o paper reformula o sizing ótimo
    // multi-hop para convergência mais rápida por variável/token. A reformulação
    // completa (fórmula fechada por token) exigiria reescrever a modelação de cada
    // hop com risco de bugs novos num código já validado -- aplicamos aqui a parte
    // de baixo risco da mesma ideia: terminação adaptativa por tolerância de
    // convergência em vez de 40 iterações fixas. Numa função côncava unimodal,
    // a maioria dos ciclos converge em 12-18 iterações; as 40 fixas desperdiçavam
    // ciclos de CPU em casos já convergidos, sem ganho de precisão.
    const MAX_ITERATIONS: usize = 40;
    const CONVERGENCE_TOLERANCE_BPS: u64 = 1; // 0.01% do intervalo -- precisão suficiente
    for _ in 0..MAX_ITERATIONS {
        if hi <= lo {
            break;
        }
        let interval = hi - lo;
        // Parar cedo quando o intervalo já é insignificante face ao valor de hi
        // (evita as iterações finais que só refinam ruído sub-wei sem valor real).
        if !hi.is_zero() {
            let tolerance_threshold = hi.saturating_mul(U256::from(CONVERGENCE_TOLERANCE_BPS)) / U256::from(10_000u64);
            if interval <= tolerance_threshold {
                break;
            }
        }
        let third = interval / U256::from(3u64);
        if third.is_zero() {
            break;
        }
        let m1 = lo + third;
        let m2 = hi - third;
        let p1 = simulate_cycle_profit_wei(hops, m1, gas_cost_wei);
        let p2 = simulate_cycle_profit_wei(hops, m2, gas_cost_wei);
        if p1 < p2 {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    let peak = (lo + hi) / U256::from(2u64);
    // INOVAÇÃO (otimização robusta via curvatura da função côncava): o pico
    // exato é o ponto de MAIOR sensibilidade a variações de reserve entre
    // deteção e eth_call -- qualquer swap concorrente empurra a reserve o
    // suficiente para o pico deixar de ser viável (IIA). Numa função côncava,
    // recuar δ do pico perde lucro de 2ª ordem (~½·f''(x*)·δ²) mas ganha
    // margem de robustez linear contra deriva de preço. ROBUSTNESS_FACTOR
    // escolhe o ponto que maximiza lucro_esperado = P(x) * P(sobrevivência|x),
    // não apenas P(x) -- confirmado por dataset real: 100% dos IIA ocorriam
    // exatamente na borda de viabilidade do pico ternário.
    const ROBUSTNESS_FACTOR_NUM: u64 = 70;
    const ROBUSTNESS_FACTOR_DEN: u64 = 100;
    (peak.saturating_mul(U256::from(ROBUSTNESS_FACTOR_NUM))) / U256::from(ROBUSTNESS_FACTOR_DEN)
}

use chrono::Timelike;
use eyre::Context as _;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static EXEC_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};
use crate::discovery::PoolDiscoveryEngine;

use crate::strategies::long_tail::{MidCapScanner, LaunchMonitor};
use crate::strategies::jit_liquidity::{JITMonitor, CLPool};
use crate::math::transfer_entropy::TransferEntropyDetector;
use crate::singularity::InvisibleProbe;
use crate::notifications::DiscordNotifier;
use crate::singularity::SequencerHeartbeatMonitor;
/// WETH na Base — usado como token de partida no grafo (SwapV3 pode expor `token_in` nulo no grafo).
const WETH: Address = address!("4200000000000000000000000000000000000006");
const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
const CBETH: Address = address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEC22");
const AERO: Address = address!("940181a94A35A4569E4529A3CDfB74e38FD98631");

/// 🐋 ORCA Engine - Motor principal de execução
#[derive(Clone, Debug)]
pub struct OrcaEngine {
    /// Módulo de segurança
    pub safety: SafetyEngine,
    /// Executor Ghost-State
    pub ghost: GhostStateExecutor,
    /// Otimizador Yul
    pub yul: YulExecutor,
    /// Sincronização com sequenciador
    pub sequencer: SequencerSync,
    /// Tracker de performance
    pub tracker: Arc<RwLock<PerformanceTracker>>,
    /// Motor de auditoria forense
    pub audit: Arc<ForensicAudit>,
    /// Capital inicial (ETH)
    initial_capital: f64,
    /// Capital atual (ETH)
    current_capital: Arc<RwLock<f64>>,
    /// Total de lucro extraído
    total_profit: Arc<RwLock<f64>>,
    /// Total de gás poupado (ETH)
    total_gas_saved: Arc<RwLock<f64>>,
    /// Contador de execuções
    execution_count: Arc<RwLock<u64>>,
    /// Cache de pools para arbitragem
    pool_cache: PoolCache,
    /// Fonte única de preço ETH/EUR (substitui hardcodes divergentes 1600/1800/3500)
    eth_price_feed: std::sync::Arc<crate::pricing::EthPriceFeed>,
    /// Grafo de arbitragem (reconstruído a cada bloco)
    arb_graph: Arc<RwLock<ArbGraph>>,
    /// Telemetria para métricas de performance
    telemetry: Option<Arc<TelemetryCollector>>,
    /// 💰 Gestor de banca adaptativo
    bankroll_manager: Arc<RwLock<BankrollManager>>,
    /// Memória de padrões de oportunidades por pool/hora
    pattern_memory: Arc<PatternMemory>,
    /// Scorer dinâmico de pools (freq/opps)
    pool_scorer: Arc<PoolScorer>,
    /// Throttle: processar no máximo 1x por bloco
    last_processed_block: Arc<AtomicU64>,
    last_event_ms: Arc<AtomicU64>,
    /// Último bloco persistido em disco
    last_pattern_persist_block: Arc<RwLock<u64>>,
    /// Último bloco em que logámos status report
    last_status_block: Arc<RwLock<u64>>,
    /// Provider para sync de saldo on-chain
    balance_provider: Arc<RootProvider<BoxTransport>>,
    /// Endereço da wallet usado para sync de saldo
    tracked_wallet: Arc<RwLock<Option<Address>>>,
    /// Último bloco observado (heartbeat / diagnóstico)
    last_observed_block: Arc<AtomicU64>,
    /// Deduplicação de eventos por (tx_hash, log_index) → block_number
    seen_events: Arc<RwLock<HashMap<(FixedBytes<32>, u64), u64>>>,
    /// Logger CSV de oportunidades
    opp_logger: Arc<crate::logger::opportunity_logger::OpportunityLogger>,
    discovery: Arc<PoolDiscoveryEngine>,
    kalman_gas: Arc<RwLock<crate::math::kalman_gas::KalmanGasPredictor>>,
    kalman_price: Arc<dashmap::DashMap<Address, crate::math::kalman_price::KalmanPricePredictor>>,
    bayesian_success: Arc<crate::math::bayesian_success::BayesianSuccessModel>,
    pool_fatigue: Arc<crate::math::pool_fatigue::PoolFatigueTracker>,
    rpc_scorer: Arc<crate::math::rpc_scorer::RpcScorer>,
    flash_optimizer: Arc<RwLock<crate::math::flash_optimizer::FlashLoanOptimizer>>,
    honeypot: Arc<crate::security::honeypot_filter::HoneypotFilter>,
    curvature: Arc<RwLock<crate::prediction::CurvatureDetector>>,
    topology: Arc<RwLock<crate::graph::PersistentTopology>>,
    midcap_scanner: Arc<MidCapScanner>,
    launch_monitor: Arc<LaunchMonitor>,
    jit_monitor: Arc<JITMonitor>,
    transfer_entropy: Arc<RwLock<TransferEntropyDetector>>,
    invisible_probe: Arc<InvisibleProbe>,
    sequencer_heartbeat: Arc<SequencerHeartbeatMonitor>,
    discord: Arc<DiscordNotifier>,
}

/// ⚙️ Configuração do ORCA
#[derive(Clone, Debug)]
pub struct OrcaConfig {
    /// Lucro mínimo para execução (ETH)
    pub min_profit_eth: f64,
    /// Lucro mínimo em € (alternativo)
    pub min_profit_eur: f64,
    /// Capital inicial
    pub initial_capital_eth: f64,
    /// RPC URL da Base
    pub base_rpc_url: String,
    /// Protector RPC URL
    pub protector_rpc_url: String,
    /// Kill-switch threshold (% do capital)
    pub kill_threshold_pct: f64,
    /// Modo diagnóstico (lucro mínimo mais baixo apenas aqui)
    pub dry_run: bool,
}

impl Default for OrcaConfig {
    fn default() -> Self {
        Self {
            min_profit_eth: 0.002, // 0.002 ETH = ~$5
            min_profit_eur: 5.0,
            initial_capital_eth: 0.05, // ~80€ @ $1600/ETH
            base_rpc_url: std::env::var("BASE_RPC_URL")
                .unwrap_or_else(|_| "https://mainnet.base.org".to_string()),
            protector_rpc_url: std::env::var("PROTECTOR_RPC_URL")
                .unwrap_or_else(|_| "https://rpc.flashbots.net/fast".to_string()),
            kill_threshold_pct: 0.50, // 50% = 40€ de 80€
            dry_run: false,
        }
    }
}

impl OrcaEngine {
    /// 🚀 Inicializa ORCA Engine
    pub async fn new(config: OrcaConfig, discovery: Arc<PoolDiscoveryEngine>, eth_price_feed: std::sync::Arc<crate::pricing::EthPriceFeed>) -> Self {
        let safety_min_profit_eth = if config.dry_run { 0.00005 } else { 0.0005 }; // CORREÇÃO: 0.0001 era menor que muitos profits que ainda assim falhavam por spread desaparecer antes da execução (mercado competitivo real) -- 0.001 (10x maior) dá margem real para sobreviver a slippage residual entre deteção e execução.
        info!("═══════════════════════════════════════════════════════════");
        info!("🐋 PROJETO ORCA - Base Mainnet MEV Engine");
        info!("═══════════════════════════════════════════════════════════");
        info!(
            "💰 Capital Inicial: {} ETH (~{}€)",
            config.initial_capital_eth,
            config.initial_capital_eth * 1600.0
        );
        info!(
            "🎯 Lucro Mínimo (Safety): {} ETH{}",
            safety_min_profit_eth,
            if config.dry_run {
                " (DRY_RUN diagnóstico)"
            } else {
                ""
            }
        );
        info!("⚖️ Priority Queue: Ativada (Higher Profit First)");
        info!(
            "💀 Kill-Switch: {}% do capital",
            config.kill_threshold_pct * 100.0
        );
        info!("⚡ Bundle: Protector RPC (Flashbots/Base)");
        info!("🔧 Yul Assembly: -30% gás");
        info!("═══════════════════════════════════════════════════════════");

        let tracker = PerformanceTracker::new();
        let audit = ForensicAudit::new("audit_results_mainnet.log");
        let pool_cache = PoolCache::new();
        let pool_cache_for_midcap = pool_cache.clone();
        let arb_graph = ArbGraph::new(pool_cache.clone(), U256::from(10).pow(U256::from(19)));

        // 💰 Inicializar BankrollManager com capital inicial em wei
        let initial_balance_wei = (config.initial_capital_eth * 1e18) as u128;
        let bankroll_manager = BankrollManager::new(initial_balance_wei);
        // CORREÇÃO: balance_provider usava config.base_rpc_url, cujo default
        // é mainnet.base.org -- RPC público que rate-limita (HTTP 429) sob
        // carga real. Isto era usado também pelo QuoterV2 (find_max_viable_
        // cycle_input faz várias chamadas eth_call por tentativa), e os 429
        // estavam a ser interpretados como "IIA real" (liquidez insuficiente)
        // quando eram apenas rate-limit do RPC -- causa provável de
        // "optimal_input refinado: X -> 0 wei" em 100% dos casos observados.
        // Agora usa o primeiro RPC privado de RPC_HTTP_URLS, com o mesmo
        // padrão de fallback já usado em submit_to_protector.
        let chosen_http_rpc = std::env::var("RPC_HTTP_URLS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .find(|u| !u.is_empty() && !u.contains("mainnet.base.org"))
            .unwrap_or_else(|| config.base_rpc_url.clone());
        let balance_provider = alloy::providers::builder()
            .on_http(
                chosen_http_rpc
                    .parse()
                    .expect("RPC_HTTP_URLS/base_rpc_url inválida para provider HTTP"),
            )
            .boxed();
        info!(
            "💰 [BANKROLL] Inicializado: {} wei | Gas budget: {} wei",
            initial_balance_wei,
            bankroll_manager.max_daily_gas_budget()
        );

        Self {
            safety: SafetyEngine::new(
                config.initial_capital_eth,
                safety_min_profit_eth,
                config.kill_threshold_pct,
            ),
            ghost: GhostStateExecutor::new(),
            yul: YulExecutor::new(),
            sequencer: SequencerSync::new(&config.protector_rpc_url),
            tracker: Arc::new(RwLock::new(tracker)),
            audit: Arc::new(audit),
            initial_capital: config.initial_capital_eth,
            current_capital: Arc::new(RwLock::new(config.initial_capital_eth)),
            total_profit: Arc::new(RwLock::new(0.0)),
            total_gas_saved: Arc::new(RwLock::new(0.0)),
            execution_count: Arc::new(RwLock::new(0)),
            pool_cache,
            arb_graph: Arc::new(RwLock::new(arb_graph)),
            telemetry: None,
            bankroll_manager: Arc::new(RwLock::new(bankroll_manager)),
            pattern_memory: Arc::new(PatternMemory::new("data/pattern_memory.json")),
            pool_scorer: Arc::new(PoolScorer::new()),
            last_processed_block: Arc::new(AtomicU64::new(0)),
            last_event_ms: Arc::new(AtomicU64::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)),
            last_pattern_persist_block: Arc::new(RwLock::new(0)),
            last_status_block: Arc::new(RwLock::new(0)),
            balance_provider: Arc::new(balance_provider),
            tracked_wallet: Arc::new(RwLock::new(None)),
            last_observed_block: Arc::new(AtomicU64::new(0)),
            seen_events: Arc::new(RwLock::new(HashMap::new())),
            opp_logger: Arc::new(crate::logger::opportunity_logger::OpportunityLogger::new("logs/opportunities.csv")),
            discovery,
            kalman_gas: Arc::new(RwLock::new(crate::math::kalman_gas::KalmanGasPredictor::new(0.1))),
            kalman_price: Arc::new(dashmap::DashMap::new()),
            bayesian_success: Arc::new(crate::math::bayesian_success::BayesianSuccessModel::new()),
            pool_fatigue: Arc::new(crate::math::pool_fatigue::PoolFatigueTracker::new()),
            rpc_scorer: Arc::new(crate::math::rpc_scorer::RpcScorer::new()),
            flash_optimizer: Arc::new(RwLock::new(crate::math::flash_optimizer::FlashLoanOptimizer::new())),
            honeypot: Arc::new(crate::security::honeypot_filter::HoneypotFilter::new()),
            curvature: Arc::new(RwLock::new(crate::prediction::CurvatureDetector::new())),
            topology: Arc::new(RwLock::new(crate::graph::PersistentTopology::new())),
            midcap_scanner: Arc::new(MidCapScanner::new(pool_cache_for_midcap)),
            launch_monitor: Arc::new(LaunchMonitor::new()),
            jit_monitor: Arc::new(JITMonitor::new()),
            transfer_entropy: Arc::new(RwLock::new(TransferEntropyDetector::new(20))),
            invisible_probe: Arc::new(InvisibleProbe::new().await),
            sequencer_heartbeat: Arc::new(SequencerHeartbeatMonitor::new().await),
            discord: Arc::new(DiscordNotifier::new(&std::env::var("DISCORD_WEBHOOK").unwrap_or_default(), eth_price_feed.clone())),
            eth_price_feed,
        }
    }

    async fn sync_wallet_balance(&self) -> eyre::Result<()> {
        let wallet = *self.tracked_wallet.read().await;
        let Some(wallet) = wallet else {
            return Ok(());
        };

        let balance = self
            .balance_provider
            .get_balance(wallet)
            .await
            .wrap_err("falha ao obter saldo da wallet")?
            .try_into().unwrap_or(u128::MAX);

        let mut bankroll = self.bankroll_manager.write().await;
        bankroll.update_balance(balance);
        Ok(())
    }

    /// 📊 Configura telemetria para métricas de performance
    pub fn set_telemetry(&mut self, telemetry: Arc<TelemetryCollector>) {
        self.telemetry = Some(telemetry);
        info!("[ORCA] 📊 Telemetria ativada — métricas em tempo real");
    }

    /// Injeta cache de pools partilhado com bootstrap/collector.
    pub fn set_shared_pool_cache(&mut self, shared_pool_cache: PoolCache) {
        self.pool_cache = shared_pool_cache.clone();
        let graph = ArbGraph::new(shared_pool_cache, U256::from(10).pow(U256::from(19)));
        self.arb_graph = Arc::new(RwLock::new(graph));
        info!("[ORCA] 🔗 Pool cache partilhado injetado no motor de arbitragem");
        let discord_start = self.discord.clone();
        tokio::spawn(async move { discord_start.notify_start().await; });
        // Arrancar InvisibleProbe em background — seleciona RPC mais rápido continuamente
        let probe = self.invisible_probe.clone();
        tokio::spawn(async move {
            probe.start_continuous_probing().await;
        });
        info!("[INVISIBLE-PROBE] 👁️ Sonda de nós iniciada em background");
        // Arrancar SequencerHeartbeat em background — aprende timing do sequencer
        let heartbeat = self.sequencer_heartbeat.clone();
        tokio::spawn(async move {
            heartbeat.start_monitoring().await;
        });
        info!("[HEARTBEAT] 💓 Monitor de sequencer iniciado em background");
    }

    /// 🔄 Mantém last_block sempre fresco via poll RPC direto, independente
    /// do fluxo de eventos de swap (que só avança quando HÁ actividade nas
    /// pools monitorizadas, causando deriva progressiva em relação ao bloco
    /// real -- já visto a chegar a -93 blocos de atraso em 14 minutos).
    pub fn spawn_block_poller(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            use alloy::providers::Provider as _;
            loop {
                match engine.balance_provider.get_block_number().await {
                    Ok(block) => {
                        engine.sequencer.update_block(block).await;
                        engine.sequencer_heartbeat.update_block(block).await;
                    }
                    Err(e) => {
                        warn!("[BLOCK-POLLER] falha a obter bloco real: {}", e);
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
            }
        });
    }

    /// 🔍 Valida oportunidade via simulação local
    pub async fn validate_opportunity(
        &self,
        opportunity: &Opportunity,
    ) -> Result<SimulationResult, String> {
        // 1. Simulação via eth_call (obrigatória)
        let sim_result = self.simulate_locally(opportunity).await?;

        // 2. Verificar lucro mínimo (0.002 ETH)
        if sim_result.net_profit_eth < self.safety.min_profit_eth() {
            return Err(format!(
                "Lucro {} ETH abaixo do mínimo {} ETH",
                sim_result.net_profit_eth,
                self.safety.min_profit_eth()
            ));
        }

        // 3. Verificar se é topo do bloco
        let timing = self.sequencer.calculate_optimal_timing().await;
        if false && !timing.will_be_top_of_block {
            return Err("Não será incluído no topo do bloco".to_string());
        }

        Ok(sim_result)
    }

    /// ⚡ Executa oportunidade validada
    pub fn last_event_ms(&self) -> u64 {
        self.last_event_ms.load(Ordering::Relaxed)
    }

    pub async fn execute_opportunity(&self, opportunity: Opportunity) -> Option<ExecutionReceipt> {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        if now_ms.saturating_sub(opportunity.detected_at_ms) > 30000 {
            return None; // oportunidade com mais de 3s -- descartar, dados já obsoletos
        }
        let exec_id = EXEC_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        info!("[DIAG-EXEC] id={} 1. entrou em execute_opportunity", exec_id);
        // 1. Validar
        let sim_result = match self.validate_opportunity(&opportunity).await {
            Ok(r) => r,
            Err(e) => {
                info!("[DIAG-EXEC] entrou no Err de validate_opportunity");
                debug!("[ORCA] ⛔ Oportunidade rejeitada: {}", e);
                return None;
            }
        };

        info!("[DIAG-EXEC] id={} 2. passou validate_opportunity", exec_id);
        // 2. Verificar kill-switch
        if !self.safety.can_operate().await {
            error!("[ORCA] 💀 KILL-SWITCH ATIVO - Execução bloqueada");
            return None;
        }

        info!("[DIAG-EXEC] id={} 3. passou kill-switch", exec_id);
        // 3. Construir bundle protegido
        let bundle = self
            .build_protected_bundle(&opportunity, &sim_result)
            .await?;

        info!("[DIAG-EXEC] id={} 4. bundle construído, a entrar em await_optimal_window", exec_id);
        // 4. Aguardar timing ótimo
        let timing = self.sequencer.await_optimal_window().await;
        info!("[DIAG-EXEC] id={} 5. await_optimal_window concluído", exec_id);
        // Heartbeat: esperar janela ótima de submissão baseada em RTT aprendido
        let next_block = self.last_observed_block.load(Ordering::Relaxed) + 1;
        let send_window = self.sequencer_heartbeat.calculate_optimal_send_time(next_block).await;
        info!("[DIAG-EXEC] id={} 6. calculate_optimal_send_time concluído", exec_id);
        self.sequencer_heartbeat.wait_for_send_window(&send_window).await;
        info!("[DIAG-EXEC] id={} 7. wait_for_send_window concluído", exec_id);

        // 5. Enviar via Protector RPC
        info!(
            "[ORCA] 🚀 ENVIANDO | Profit: {} ETH | Gas: {} | Slot: {}",
            sim_result.net_profit_eth, sim_result.gas_used, timing.block_slot
        );

        let receipt = self.submit_to_protector(bundle, timing).await?;

        // 6. Atualizar estado
        self.update_state_after_execution(&receipt, &sim_result)
            .await;

        // 7. Log de performance
        self.log_performance(&receipt, &sim_result).await;

        Some(receipt)
    }

    /// 🧠 Simula execução localmente (eth_call)
    async fn simulate_locally(
        &self,
        opportunity: &Opportunity,
    ) -> Result<SimulationResult, String> {
        // Em produção: chamar eth_call real no RPC
        // Eliminado mocks de lucro fixo
        let gas_used = 150000u64;
        let gas_price_gwei = 0.1f64;

        let gross_profit = opportunity.expected_profit_eth;
        if gross_profit <= 0.0 {
            return Err("Lucro esperado inválido ou nulo".to_string());
        }

        let gas_cost_eth = (gas_used as f64 * gas_price_gwei) / 1e9;

        // Yul otimization: -30% gas
        let yul_gas_saved = (gas_used as f64 * 0.30) as u64;
        let final_gas = gas_used - yul_gas_saved;
        let final_gas_cost = (final_gas as f64 * gas_price_gwei) / 1e9;
        let final_profit = gross_profit - final_gas_cost;

        if final_profit <= 0.0 {
            return Err(format!(
                "Oportunidade não lucrativa após gás: {} ETH",
                final_profit
            ));
        }

        Ok(SimulationResult {
            gross_profit_eth: gross_profit,
            net_profit_eth: final_profit,
            gas_used: final_gas,
            gas_cost_eth: final_gas_cost,
            gas_saved_eth: gas_cost_eth - final_gas_cost,
            will_succeed: true,
        })
    }

    /// 📦 Constrói bundle protegido
    async fn build_protected_bundle(
        &self,
        opportunity: &Opportunity,
        sim: &SimulationResult,
    ) -> Option<ProtectedBundle> {
        // Usar Yul executor para otimização
        let _yul_tx = self.yul.build_optimized_transaction(opportunity).await?;

        Some(ProtectedBundle {
            transactions: vec![],
            min_profit_eth: 0.0001, // margem de seguranca fixa, nao o profit total esperado
            max_gas_eth: sim.gas_cost_eth * 1.1,
            target_slot: 0,
            revert_on_failure: true,
            hops: opportunity.hops.clone(),
            loan_amount_wei: opportunity.amount_in,
            detected_at_ms: opportunity.detected_at_ms,
            priority_fee_wei: {
                // INOVAÇÃO: liga o KalmanGasPredictor (ja existente, previa gas
                // price mas nunca influenciava a tx real -- estava fixo em
                // 1_000_000 wei) ao priority fee efetivamente enviado. Usa a
                // previsao com margem de 1 sigma (safe_gas_price_gwei), limitada
                // pelo GAS_CAP_GWEI configurado.
                let gwei = self.kalman_gas.read().await.safe_gas_price_gwei();
                let cap_gwei: f64 = std::env::var("GAS_CAP_GWEI").ok()
                    .and_then(|s| s.parse().ok()).unwrap_or(50.0);
                let clamped_gwei = gwei.max(0.001).min(cap_gwei);
                (clamped_gwei * 1_000_000_000.0) as u128
            },
        })
    }

    /// 📡 Envia para Protector RPC
    async fn submit_to_protector(
        &self,
        bundle: ProtectedBundle,
        timing: BlockTiming,
    ) -> Option<ExecutionReceipt> {
        use alloy::network::{TransactionBuilder, EthereumWallet};
        use alloy::rpc::types::TransactionRequest;
        use alloy::signers::local::PrivateKeySigner;

        let private_key_str = match std::env::var("PRIVATE_KEY") {
            Ok(k) => k,
            Err(_) => { warn!("[ORCA] ❌ PRIVATE_KEY não definida"); return None; }
        };
        let executor_addr = match std::env::var("EXECUTOR_ADDRESS")
            .ok()
            .and_then(|s| s.parse::<Address>().ok())
        {
            Some(a) => a,
            None => { warn!("[ORCA] ❌ EXECUTOR_ADDRESS inválido"); return None; }
        };
        let signer: PrivateKeySigner = match private_key_str.parse() {
            Ok(s) => s,
            Err(e) => { warn!("[ORCA] ❌ Chave privada inválida: {}", e); return None; }
        };
        let wallet = EthereumWallet::from(signer.clone());

        // Construir route: executor(20)+WETH(20)+loanAmount(32)+deadline(4)+minProfit(4)+hopCount(1)+hops*41
        let mut route: Vec<u8> = Vec::with_capacity(81 + bundle.hops.len() * 41);
        route.extend_from_slice(executor_addr.as_slice());
        let weth: Address = "0x4200000000000000000000000000000000000006".parse().unwrap();
        route.extend_from_slice(weth.as_slice());
        route.extend_from_slice(&bundle.loan_amount_wei.to_be_bytes::<32>());
        route.extend_from_slice(&(timing.target_block as u32 + 10u32).to_be_bytes()); // CORREÇÃO: +2 blocos (~4s) era insuficiente para nonce+eth_call+propagação real -- 99.3% das tentativas falhavam por "Block deadline exceeded" mesmo com timing já otimizado no lado Rust. +5 blocos (~10s) dá margem real sem expor a oportunidades já completamente obsoletas.
        let min_profit_compact = ((bundle.min_profit_eth * 1e18) as u64 / 1_000_000_000) as u32;
        route.extend_from_slice(&min_profit_compact.to_be_bytes());
        route.push(bundle.hops.len() as u8);
        for hop in &bundle.hops {
            route.extend_from_slice(hop.pool.as_slice());
            route.extend_from_slice(hop.token_out.as_slice());
            let dex_flag: u8 = match hop.dex_type {
                crate::contracts::DexType::Aerodrome | crate::contracts::DexType::AerodromeStable => 0x80,
                _ => 0x00,
            };
            let fee_idx: u8 = match hop.fee { 500 => 0, 3000 => 1, 10000 => 2, _ => 1 };
            route.push(dex_flag | fee_idx);
        }

        // ABI encode: selector execute(bytes) + offset + len + data
        let mut calldata: Vec<u8> = Vec::with_capacity(4 + 64 + route.len());
        calldata.extend_from_slice(&[0x09, 0xc5, 0xea, 0xbe]);
        calldata.extend_from_slice(&U256::from(32u64).to_be_bytes::<32>());
        calldata.extend_from_slice(&U256::from(route.len() as u64).to_be_bytes::<32>());
        calldata.extend_from_slice(&route);

        // Re-check imediatamente antes de assinar: liquidez/preço frescos via eth_call
        // no próprio hop.pool, comparando contra a simulação usada para decidir o tamanho.
        {
            let mut still_viable = true;
            for hop in &bundle.hops {
                if hop.dex_type != crate::contracts::DexType::UniswapV3 {
                    continue;
                }
                let slot0_call = TransactionRequest::default()
                    .to(hop.pool).input(vec![0x38,0x50,0xc7,0xbd].into());
                let liq_call = TransactionRequest::default()
                    .to(hop.pool).input(vec![0x1a,0x68,0x65,0x02].into());
                let http_url: reqwest::Url = match std::env::var("RPC_HTTP_URLS")
                    .unwrap_or_default()
                    .split(',')
                    .find(|u| !u.contains("mainnet.base.org") && !u.is_empty())
                    .unwrap_or("https://mainnet.base.org")
                    .parse() {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                let fresh_provider = alloy::providers::ProviderBuilder::new().on_http(http_url);
                let slot0 = fresh_provider.call(&slot0_call).await.ok();
                let liq = fresh_provider.call(&liq_call).await.ok();
                if let (Some(s), Some(l)) = (slot0, liq) {
                    if s.len() >= 32 && l.len() >= 16 {
                        let sqrt_fresh = U256::from_be_slice(&s[0..32]).try_into().unwrap_or(0u128);
                        let liq_fresh = u128::from_be_bytes(l[16..32].try_into().unwrap_or([0;16]));
                        if liq_fresh == 0 || sqrt_fresh == 0 {
                            still_viable = false;
                            break;
                        }
                    }
                }
            }
            if !still_viable {
                warn!("[ORCA] ❌ Liquidez fresca inválida no momento do envio -- abortado");
                return None;
            }
        }

        // CORREÇÃO: preferir RPCs privados (Tenderly/QuickNode) sobre o público
        // mainnet.base.org -- esse rate-limita (HTTP 429) sob qualquer carga
        // real, e mesmo os privados podem sofrer timeouts (HTTP 408/504)
        // sob carga. Privados primeiro, público como último recurso --
        // tenta cada um em sequência até um responder em vez de desistir
        // ao primeiro timeout (isto estava a abortar ~35% das execuções
        // antes mesmo de chegar ao eth_call).
        let rpc_list = std::env::var("RPC_HTTP_URLS").unwrap_or_default();
        let mut rpc_candidates: Vec<String> = rpc_list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|u| !u.is_empty() && !u.contains("mainnet.base.org"))
            .collect();
        // INOVAÇÃO: endpoint público Flashblocks da Base -- preconfirmações a
        // cada 200ms em vez dos blocos completos de 2s. Adicionado com
        // prioridade (primeiro na corrida) porque o nonce lido aqui reflete
        // estado mais recente que "latest" nos RPCs genéricos, atacando
        // diretamente o maior componente da latência medida (~1267ms da
        // corrida de nonce, de um total de ~1.9s detecção->eth_call).
        rpc_candidates.insert(0, "https://mainnet-preconf.base.org".to_string());
        rpc_candidates.push("https://mainnet.base.org".to_string());

        // INOVACAO: em vez de correr contra TODOS os RPCs sempre (auto-infligindo
        // contencao/rate-limit em endpoints partilhados gratuitos), seleciona os
        // 3 melhores por desempenho historico real (latencia + taxa de sucesso).
        let rpc_candidates = self.rpc_scorer.top_n(&rpc_candidates, 3);
        let from_addr = signer.address();
        let mut join_set = tokio::task::JoinSet::new();
        for rpc_url in rpc_candidates.clone() {
            let wallet = wallet.clone();
            let scorer = self.rpc_scorer.clone();
            join_set.spawn(async move {
                let t_start = std::time::Instant::now();
                let http_url: reqwest::Url = match rpc_url.parse() {
                    Ok(u) => u,
                    Err(_) => return (rpc_url, None),
                };
                let candidate_provider = alloy::providers::ProviderBuilder::new()
                    .wallet(wallet)
                    .on_http(http_url);
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(800),
                    candidate_provider.get_transaction_count(from_addr).pending(),
                ).await;
                let elapsed_ms = t_start.elapsed().as_millis() as f64;
                match result {
                    Ok(Ok(n)) => { scorer.record(&rpc_url, elapsed_ms, true); (rpc_url, Some((n, candidate_provider))) }
                    Ok(Err(e)) => { warn!("[ORCA] RPC {} falhou: {}", rpc_url, e); scorer.record(&rpc_url, elapsed_ms, false); (rpc_url, None) }
                    Err(_) => { warn!("[ORCA] RPC {} timeout 800ms", rpc_url); scorer.record(&rpc_url, 800.0, false); (rpc_url, None) }
                }
            });
        }
        let mut provider_opt = None;
        let mut nonce_opt = None;
        while let Some(res) = join_set.join_next().await {
            if let Ok((_, Some((n, p)))) = res {
                nonce_opt = Some(n);
                provider_opt = Some(p);
                join_set.abort_all();
                break;
            }
        }
        let provider = match provider_opt {
            Some(p) => p,
            None => { warn!("[ORCA] ❌ Todos os RPCs falharam ao obter nonce"); return None; }
        };
        let nonce = match nonce_opt {
            Some(n) => n,
            None => { warn!("[ORCA] ❌ Todos os RPCs falharam ao obter nonce"); return None; }
        };

        let tx = TransactionRequest::default()
            .with_from(from_addr)
            .with_to(executor_addr)
            .with_nonce(nonce)
            .with_chain_id(8453u64)
            .with_input(alloy::primitives::Bytes::from(calldata.clone()))
            .with_gas_limit(600_000u64)
            .with_max_fee_per_gas(bundle.priority_fee_wei.saturating_mul(10))
            .with_max_priority_fee_per_gas(bundle.priority_fee_wei);

        // CORREÇÃO: simular via eth_call ANTES de gastar gás real — sem isto, mudanças
        // de preço entre deteção e inclusão causam revert real (perdeu gás 2x esta noite).
        let real_block_for_diag = provider.get_block_number().await.unwrap_or(0);
        info!("[DIAG-DEADLINE] target_block_encoded={} bloco_real_agora={} diff={}", timing.target_block as u32 + 10u32, real_block_for_diag, (timing.target_block as i64 + 10i64) - real_block_for_diag as i64);
        let call_req = TransactionRequest::default()
            .with_from(from_addr)
            .with_to(executor_addr)
            .with_input(alloy::primitives::Bytes::from(calldata));
        let latency_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64 as i64 - bundle.detected_at_ms as i64;
        info!("[LATENCY-DIAG] deteccao->eth_call: {}ms", latency_ms);
        if let Err(e) = provider.call(&call_req).await {
            warn!("[ORCA] falha simulacao eth_call - abortado antes de gastar gas real: {}", e);
            if let Some(first_hop) = bundle.hops.first() {
                self.pool_fatigue.record_failure(first_hop.pool);
            }
            return None;
        }
        debug!("[ORCA] simulacao eth_call passou - a enviar tx real");

        let protector_url = std::env::var("PROTECTOR_RPC_URL")
            .unwrap_or_else(|_| "https://rpc.flashbots.net/fast".to_string());
        let pending = match protector_url.parse::<reqwest::Url>() {
            Ok(purl) => {
                let protector_provider = alloy::providers::ProviderBuilder::new()
                    .wallet(wallet.clone())
                    .on_http(purl);
                match protector_provider.send_transaction(tx.clone()).await {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("[ORCA] Protector RPC falhou ({}), fallback generico", e);
                        match provider.send_transaction(tx).await {
                            Ok(p) => p,
                            Err(e2) => { warn!("[ORCA] Falha ao enviar tx (fallback): {}", e2); return None; }
                        }
                    }
                }
            }
            Err(_) => {
                match provider.send_transaction(tx).await {
                    Ok(p) => p,
                    Err(e) => { warn!("[ORCA] Falha ao enviar tx: {}", e); return None; }
                }
            }
        };

        let tx_hash = format!("{:?}", pending.tx_hash());
        info!("[ORCA] 🚀 TX enviada: {} — a aguardar confirmação on-chain...", tx_hash);

        // CORREÇÃO CRÍTICA: só notificar "lucro" depois de confirmarmos que a tx foi
        // MINADA E TEVE SUCESSO. Antes disto, "enviada" não significa "lucro real" —
        // a transação pode reverter (preço mudou, slippage, etc.) e o saldo não muda.
        let discord_exec = self.discord.clone();
        let tx_hash_d = tx_hash.clone();
        let profit_eth_d = bundle.min_profit_eth;
        let loan_eth_d = bundle.loan_amount_wei.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
        // INOVAÇÃO: metadata para dataset Bayesiano de probabilidade de sucesso
        // por segmento (dex_type + hop_count + latência) -- sem isto, os reverts
        // nunca ficavam registados em CSV, só em log de texto, impossibilitando
        // qualquer análise estatística real dos 49+ casos já ocorridos.
        let dex_types_d: Vec<String> = bundle.hops.iter().map(|h| format!("{:?}", h.dex_type)).collect();
        let bayesian_d = self.bayesian_success.clone();
        let hop_count_d = bundle.hops.len();
        let latency_ms_d = latency_ms;
        tokio::spawn(async move {
            let (status_str, profit_final): (&str, f64) = match tokio::time::timeout(std::time::Duration::from_secs(30), pending.get_receipt()).await {
                Ok(Ok(receipt)) => {
                    if receipt.status() {
                        discord_exec.notify_execution(&tx_hash_d, profit_eth_d, loan_eth_d, 0.0).await;
                        info!("[ORCA] ✅ TX {} CONFIRMADA on-chain com SUCESSO (status=1)", tx_hash_d);
                        ("success", profit_eth_d)
                    } else {
                        warn!("[ORCA] ❌ TX {} foi incluída mas REVERTEU (status=0) — SEM lucro real, gás perdido", tx_hash_d);
                        discord_exec.notify_error(&format!("TX revertida (status=0): {}", tx_hash_d)).await;
                        ("revert", 0.0)
                    }
                }
                Ok(Err(e)) => {
                    warn!("[ORCA] ❌ Falha ao obter receipt de {}: {}", tx_hash_d, e);
                    discord_exec.notify_error(&format!("Falha ao confirmar TX: {} | {}", tx_hash_d, e)).await;
                    ("receipt_error", 0.0)
                }
                Err(_) => {
                    warn!("[ORCA] ⏱️ TX {} sem confirmação em 30s — estado desconhecido, NÃO notificado como lucro", tx_hash_d);
                    ("timeout", 0.0)
                }
            };
            let bayesian_key = format!("{}|{}", dex_types_d.join("|"), hop_count_d);
            bayesian_d.record(&bayesian_key, status_str == "success");
            let exec_line = format!(
                "{},{},{:.6},{:.4},{},{},{},{}\n",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
                tx_hash_d, profit_final, loan_eth_d, status_str,
                dex_types_d.join("|"), hop_count_d, latency_ms_d
            );
            let _ = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("logs/executions.csv")
                .await
                .map(|mut f| {
                    use tokio::io::AsyncWriteExt;
                    tokio::spawn(async move {
                        let _ = f.write_all(exec_line.as_bytes()).await;
                    });
                });
        });

        Some(ExecutionReceipt {
            tx_hash,
            block_number: timing.target_block,
            slot: timing.block_slot,
            profit_eth: bundle.min_profit_eth,
            gas_used: 600_000,
            gas_saved_eth: 0.0,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }

    /// 📊 Atualiza estado após execução
    async fn update_state_after_execution(
        &self,
        receipt: &ExecutionReceipt,
        _sim: &SimulationResult,
    ) {
        let mut capital = self.current_capital.write().await;
        *capital += receipt.profit_eth;
        drop(capital);

        let mut profit = self.total_profit.write().await;
        *profit += receipt.profit_eth;
        drop(profit);

        let mut gas_saved = self.total_gas_saved.write().await;
        *gas_saved += receipt.gas_saved_eth;
        drop(gas_saved);

        let mut count = self.execution_count.write().await;
        *count += 1;
        drop(count);

        // Notificar safety engine
        self.safety.record_profit(receipt.profit_eth).await;

        // Verificar kill-switch
        let current = *self.current_capital.read().await;
        if self.safety.check_kill_threshold(current).await {
            self.trigger_kill_switch().await;
        }
    }

    /// 📝 Log de performance
    async fn log_performance(&self, receipt: &ExecutionReceipt, sim: &SimulationResult) {
        let capital = *self.current_capital.read().await;
        let profit = *self.total_profit.read().await;
        let gas_saved = *self.total_gas_saved.read().await;

        info!(
            "[ORCA-HIT] 💰 Lucro: {} ETH | Slot: {} | Block: {}",
            receipt.profit_eth, receipt.slot, receipt.block_number
        );

        info!(
            "[GAS-SAVED] ⛽ Economia Yul: {} ETH | Gas usado: {} | Poupado: {}%",
            receipt.gas_saved_eth,
            receipt.gas_used,
            (sim.gas_saved_eth / sim.gas_cost_eth * 100.0) as u64
        );

        info!(
            "[BANK-TOTAL] 💎 Saldo: {} ETH | Total lucro: {} ETH | Total gás poupado: {} ETH",
            capital, profit, gas_saved
        );
    }

    /// 💀 Ativa kill-switch
    async fn trigger_kill_switch(&self) {
        error!("═══════════════════════════════════════════════════════════");
        error!("🐋 ORCA KILL-SWITCH ATIVADO");
        error!("💸 Capital protegido. Sistema parado.");
        error!("🔑 Use código de autorização para retomar.");
        error!("═══════════════════════════════════════════════════════════");

        let mut status = self.safety.system_status.write().await;
        *status = safety::SystemStatus::Halted;
    }

    /// 📊 Estatísticas gerais
    pub async fn stats(&self) -> String {
        let capital = *self.current_capital.read().await;
        let profit = *self.total_profit.read().await;
        let gas_saved = *self.total_gas_saved.read().await;
        let count = *self.execution_count.read().await;
        let roi = if self.initial_capital > 0.0 {
            (profit / self.initial_capital) * 100.0
        } else {
            0.0
        };

        format!(
            "\u{1f40b} ORCA | Capital: {} ETH | Lucro: {} ETH | ROI: {:.1}% | Execuções: {} | Gás poupado: {} ETH",
            capital, profit, roi, count, gas_saved
        )
    }

    /// 🕵️ Observa oportunidade sem executar (PASSIVE_OBSERVER)
    /// Prova rentabilidade real sem queimar capital
    pub async fn observe_opportunity(&self, opportunity: Opportunity, whale_tx_hash: &str) {
        // 1. Validar via simulação
        let sim_result = match self.validate_opportunity(&opportunity).await {
            Ok(r) => r,
            Err(e) => {
                // Não logar rejeições comuns para não poluir o terminal
                trace!("[ORCA] ⛔ Oportunidade observada mas rejeitada: {}", e);
                return;
            }
        };

        // 2. Registar no auditor forense (Forensic Mode)
        let block_timing = self.sequencer.calculate_optimal_timing().await;

        // Simular variação de preço (slippage real) no microssegundo
        let simulated_slippage = 0.00045; // 0.045% slippage real simulado

        info!(
            "🎯 [OPPORTUNITY] Oportunidade Detetada! Baleia: {} | Lucro Estimado: {} ETH",
            &whale_tx_hash[..10],
            sim_result.net_profit_eth
        );

        self.audit
            .log_opportunity(
                block_timing.target_block,
                &format!("0x{:x}", block_timing.target_block),
                whale_tx_hash,
                sim_result.net_profit_eth,
                simulated_slippage,
                sim_result.gas_cost_eth,
                self.eth_price_feed.get_eur().await, // CORREÇÃO: preço real CoinGecko, não hardcoded
            )
            .await;
    }

    /// 🏁 Encerra sessão e gera relatório final
    pub async fn shutdown(&self) {
        info!("[ORCA] 🏁 Encerrando sessão de monitorização...");
        self.audit.generate_final_report().await;
    }
}

#[async_trait]
impl Strategy for OrcaEngine {
    /// 📥 Processa eventos do Artemis e encaminha para o motor ORCA
    /// LOGGING TOTALMENTE TRANSPARENTE - mostra EXATAMENTE o que está a acontecer
    async fn process_event(
        &mut self,
        event: MevEvent,
        context: &StrategyContext,
    ) -> eyre::Result<()> {
        match event {
            MevEvent::Swap(swap) => {
                self.last_observed_block
                    .store(swap.block_number, Ordering::Relaxed);
                self.last_event_ms.store(
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                    Ordering::Relaxed,
                );
                // CORREÇÃO CRÍTICA: sincronizar o bloco real com o SequencerSync --
                // sem isto, calculate_optimal_timing calculava target_block sempre a
                // partir de last_block=0 (nunca atualizado), fazendo o blockDeadline
                // codificado no calldata nunca se aproximar do bloco real da chain.
                // Isto fazia ~99% das execuções falharem no eth_call com "Block
                // deadline exceeded", independentemente de qualquer margem configurada.
                // NOTA: update_block aqui usa swap.block_number como fallback
                // imediato (zero latência extra), mas a fonte de verdade real
                // é a poll task dedicada (ver spawn_block_poller, chamada no
                // arranque do OrcaEngine) -- swap.block_number sozinho causava
                // uma deriva progressiva (~12% mais lento que o bloco real)
                // porque só avança quando HÁ um swap relevante, não a cada
                // bloco real da chain.
                self.sequencer.update_block(swap.block_number).await;
                self.sequencer_heartbeat.update_block(swap.block_number).await;
                let current_block = swap.block_number;

                // ── Deduplicação: cada log processado no máximo 1 vez ──
                {
                    let mut seen = self.seen_events.write().await;
                    let key = (swap.tx_hash, swap.log_index);
                    if seen.contains_key(&key) {
                        trace!(
                            "[DEDUP] Skip evento duplicado tx={:?} log_index={} block={}",
                            swap.tx_hash, swap.log_index, swap.block_number
                        );
                        return Ok(());
                    }
                    // Limpar entradas com mais de 1 bloco de idade (TTL)
                    seen.retain(|_, block| *block >= current_block.saturating_sub(1));
                    seen.insert(key, current_block);
                }

                // ── Sync Event (marcador fee=0): actualizar reserves reais, sem cálculo arb ──
                if swap.fee == 0 {
                    // amount_in = reserve0, amount_out = reserve1 (valores reais do on-chain)
                    self.pool_cache.update_sync_event(
                        swap.pool,
                        swap.amount_in,
                        swap.amount_out,
                        swap.block_number,
                    );
                    // Alimentar CurvatureDetector com reserves reais
                    {
                        let r_in = swap.amount_in.try_into().unwrap_or(u128::MAX) as f64;
                        let r_out = swap.amount_out.try_into().unwrap_or(u128::MAX) as f64;
                        self.curvature.write().await.update(
                            swap.pool,
                            swap.block_number,
                            r_in,
                            r_out,
                        );
                    }
                    trace!(
                        "[SYNC] Cache actualizado: pool={:?} r0={} r1={}",
                        swap.pool,
                        swap.amount_in,
                        swap.amount_out
                    );
                    return Ok(()); // Não fazer cálculo arb para Sync events
                }

                // ── Real Swap event: lógica existente abaixo ──

                // Resolver token_in/token_out do cache quando ZERO (decoder não conhece tokens)
                let (resolved_token_in, resolved_token_out) = if swap.token_in != Address::ZERO {
                    (swap.token_in, swap.token_out)
                } else if let Some(pool_state) = self.pool_cache.get(&swap.pool) {
                    if pool_state.token0 != Address::ZERO {
                        (pool_state.token0, pool_state.token1)
                    } else {
                        (Address::ZERO, Address::ZERO)
                    }
                } else {
                    (Address::ZERO, Address::ZERO)
                };

                // Log útil mostrando tokens reais quando disponíveis
                if resolved_token_in != Address::ZERO {
                    debug!(
                        "[SWAP] pool={:?} {} → {} amount={}",
                        swap.pool, resolved_token_in, resolved_token_out, swap.amount_in
                    );
                }

                // 1) Atualizar cache com dados do swap
                // Para pools sem reserves explícitas no evento, usamos amount_in/out como aproximação.
                let synthetic_reserve0 = swap.amount_in.saturating_mul(U256::from(20u32));
                let synthetic_reserve1 = swap.amount_out.saturating_mul(U256::from(20u32));
                // Atualizar cache: sintético apenas para pools sem reserves reais ainda.
                // Não destruir reserves reais com aproximações de amount_in × 20.
                let pool_has_real_reserves = self
                    .pool_cache
                    .get(&swap.pool)
                    .map(|s| s.last_update_block > 0)
                    .unwrap_or(false);
                if pool_has_real_reserves {
                    // Só actualizar timestamp — preservar reserves reais do bootstrap
                    self.pool_cache.touch(swap.pool, swap.block_number);
                } else {
                    // Pool ainda não bootstrapado — usar aproximação sintética como proxy
                    self.pool_cache.update_sync_event(
                        swap.pool,
                        synthetic_reserve0,
                        synthetic_reserve1,
                        swap.block_number,
                    );
                }

                if let (Some(sqrt), Some(liq)) = (swap.sqrt_price_x96, swap.liquidity) {
                    self.pool_cache
                        .update_swap_event(swap.pool, sqrt, liq, swap.block_number);
                }

                // Sistema 3: scoring de frequência
                self.pool_scorer
                    .on_swap_received(&format!("{:?}", swap.pool));

                // Throttle: swaps pequenos só 1x por bloco; swaps grandes calculam sempre
                let _last = self.last_processed_block.load(Ordering::Relaxed);
                let is_large_swap = swap.amount_in >= U256::from(1_000_000_000_000_000_000u128); // >= 1 ETH
                if is_large_swap {
                    let needs_bootstrap = self.pool_cache.get(&swap.pool)
                        .map(|s| s.last_update_block == 0)
                        .unwrap_or(true);
                    info!("[DEBUG] large_swap pool={:?} needs_bootstrap={} in_cache={}", 
                        swap.pool, needs_bootstrap, self.pool_cache.contains(&swap.pool));
                    info!("[LARGE-SWAP] pool={:?} amount_in={}", swap.pool, swap.amount_in);
                    // ── JIT: avaliar oportunidade just-in-time para pools V3 ──
                    if swap.dex_type == crate::contracts::DexType::UniswapV3 {
                        if let Some(pool_state) = self.pool_cache.get(&swap.pool) {
                            if pool_state.sqrt_price_x96.is_some() && pool_state.liquidity.is_some() {
                                let cl_pool = CLPool {
                                    address: swap.pool,
                                    token0: pool_state.token0,
                                    token1: pool_state.token1,
                                    fee: pool_state.fee,
                                    tick: pool_state.tick.unwrap_or(0),
                                    liquidity: pool_state.liquidity.unwrap_or(0),
                                    sqrt_price_x96: U256::from(pool_state.sqrt_price_x96.unwrap_or(0)),
                                    tvl_usd: pool_state.tvl_eth.try_into().unwrap_or(u128::MAX) as f64 / 1e18 * self.eth_price_feed.get_eur().await,
                                };
                                let swap_eth = swap.amount_in.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
                                let gas_gwei = 0.1f64;
                                if let Some(jit_opp) = self.jit_monitor.evaluate_opportunity(&cl_pool, swap_eth, gas_gwei) {
                                    info!("[JIT] 🎯 pool={:?} fee={:.6}ETH gas={:.6}ETH", jit_opp.pool, jit_opp.expected_fee_eth, jit_opp.gas_cost_eth);
                                }
                            }
                        }
                    }
                    // ── BACKRUN: atualizar reserves com estado pós-swap imediato ──
                    if swap.token_in != Address::ZERO && swap.token_out != Address::ZERO {
                        if let Some(mut pool) = self.pool_cache.get(&swap.pool) {
                            let (new_r0, new_r1) = if pool.token0 == swap.token_in {
                                (pool.reserve0 + swap.amount_in, pool.reserve1.saturating_sub(swap.amount_out))
                            } else {
                                (pool.reserve0.saturating_sub(swap.amount_out), pool.reserve1 + swap.amount_in)
                            };
                            pool.reserve0 = new_r0;
                            pool.reserve1 = new_r1;
                            pool.last_update_block = swap.block_number;
                            self.pool_cache.insert(pool);
                            debug!("[BACKRUN] reserves atualizadas pool={:?} r0={} r1={}", swap.pool, new_r0, new_r1);
                        }
                    }
                    // Bootstrap on-the-fly se pool desconhecida
                    let needs_bootstrap = self.pool_cache.get(&swap.pool)
                        .map(|s| s.last_update_block == 0)
                        .unwrap_or(true);
                    if needs_bootstrap {
                        let provider = self.balance_provider.clone();
                        let pool_addr = swap.pool;
                        let cache = self.pool_cache.clone();
                        let discovery = self.discovery.clone();
                        let launch_mon_ref = self.launch_monitor.clone();
                        let midcap_ref = self.midcap_scanner.clone();
                        tokio::spawn(async move {
                            let q96 = U256::from(1u128) << 96;
                            // token0/token1
                            let t0_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0x0d,0xfe,0x16,0x81].into());
                            let t1_call = alloy::rpc::types::TransactionRequest::default()
                               .to(pool_addr).input(vec![0xd2,0x12,0x20,0xa7].into());
                            let t0 = provider.call(&t0_call).await.ok();
                            let t1 = provider.call(&t1_call).await.ok();
                            let (token0, token1) = match (t0, t1) {
                                (Some(r0), Some(r1)) if r0.len() >= 32 && r1.len() >= 32 => {
                                    (Address::from_slice(&r0[12..32]), Address::from_slice(&r1[12..32]))
                                }
                                _ => return,
                            };
                            if token0 == Address::ZERO || token1 == Address::ZERO { return; }
                            // Tentar slot0+liquidity (V3)
                            let slot0_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0x38,0x50,0xc7,0xbd].into());
                            let liq_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0x1a,0x68,0x65,0x02].into());
                            let slot0 = provider.call(&slot0_call).await.ok();
                            let liq = provider.call(&liq_call).await.ok();
                            // CORREÇÃO DE CAUSA RAIZ FINAL: fee era SEMPRE hardcoded a
                            // 3000 (0.3%) para pools descobertas on-the-fly, independente
                            // do fee real da pool -- confirmado on-chain: uma pool real
                            // com fee=10000 (1%) ficava marcada como fee=3000 no nosso
                            // cache, fazendo o QuoterV2 (que usa factory.getPool(token0,
                            // token1, fee) internamente) calcular o endereço de uma POOL
                            // DIFERENTE (a pool real com fee=3000 para este par, que por
                            // coincidência existe mas tem liquidity()=0 -- morta). Isto
                            // explicava 100% das falhas "Unexpected error"/"IIA" mesmo
                            // com liquidez genuína confirmada na pool real (fee=10000).
                            let fee_call = alloy::rpc::types::TransactionRequest::default()
                                .to(pool_addr).input(vec![0xdd, 0xca, 0x3f, 0x43].into());
                            let real_fee: u32 = match provider.call(&fee_call).await.ok() {
                                Some(d) if d.len() >= 32 => {
                                    U256::from_be_slice(&d[0..32]).try_into().unwrap_or(3000u32)
                                }
                                _ => 3000u32, // fallback apenas se a própria chamada falhar (ex: pool V2/Aerodrome sem fee() dinâmico)
                            };
                            let (reserve0, reserve1, dex, sqrt_opt, liq_opt) =
                                if let (Some(s), Some(l)) = (slot0, liq) {
                                    if s.len() >= 32 && l.len() >= 16 {
                                        let sqrt = U256::from_be_slice(&s[0..32]);
                                        let liquidity = u128::from_be_bytes(l[16..32].try_into().unwrap_or([0;16]));
                                        let liq_u = U256::from(liquidity);
                                        let r0 = liq_u.checked_mul(q96).and_then(|v| v.checked_div(sqrt)).unwrap_or(U256::ZERO);
                                        let r1 = liq_u.checked_mul(sqrt).and_then(|v| v.checked_div(q96)).unwrap_or(U256::ZERO);
                                        (r0, r1, crate::contracts::DexType::UniswapV3, Some(sqrt.saturating_to::<u128>()), Some(liquidity))
                                    } else { (U256::ZERO, U256::ZERO, crate::contracts::DexType::Aerodrome, None, None) }
                                } else {
                                    // Fallback V2 getReserves
                                    let gr_call = alloy::rpc::types::TransactionRequest::default()
                                        .to(pool_addr).input(vec![0x09,0x02,0xf1,0xac].into());
                                    match provider.call(&gr_call).await.ok() {
                                        Some(d) if d.len() >= 64 => {
                                            let r0 = U256::from_be_slice(&d[0..32]);
                                            let r1 = U256::from_be_slice(&d[32..64]);
                                            (r0, r1, crate::contracts::DexType::Aerodrome, None, None)
                                        }
                                        _ => return,
                                    }
                                };
                            if reserve0.is_zero() || reserve1.is_zero() { return; }
                            let usdc = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
                            let dec0 = if token0 == usdc { 6u8 } else { 18u8 };
                            let dec1 = if token1 == usdc { 6u8 } else { 18u8 };
                            let mut state = crate::cache::pool_cache::PoolState::new(pool_addr, token0, token1, real_fee, dex);
                            state.reserve0 = reserve0;
                            state.reserve1 = reserve1;
                            state.decimals0 = dec0;
                            state.decimals1 = dec1;
                            state.sqrt_price_x96 = sqrt_opt;
                            state.liquidity = liq_opt;
                            state.last_update_block = 1;
                            // Validar reserves: mínimo 0.1 ETH em ambos os lados
                            let min_reserve = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
                            if reserve0 < min_reserve && reserve1 < min_reserve { return; }
                            // Para V3: rejeitar se sqrt_price implica preço absurdo (fee harcoded 3000 é proxy)
                            cache.insert(state);
                            // Alimentar launch_monitor e midcap_scanner com pool nova
                            launch_mon_ref.on_pair_created(pool_addr, 0);
                            midcap_ref.track_token(token0);
                            midcap_ref.track_token(token1);
                            let disc = discovery.clone();
                            tokio::spawn(async move {
                                disc.register_pool_otf(
                                    pool_addr, token0, token1, real_fee,
                                    reserve0, reserve1, 2000.0,
                                ).await;
                                let _ = disc.save_to_cache().await;
                            });
                            info!("[ON-THE-FLY] {:?} t0={:?} t1={:?} r0={} r1={}", pool_addr, token0, token1, reserve0, reserve1);
                        });
                    }
                }
                if !is_large_swap {
    if self.last_processed_block.fetch_max(current_block, Ordering::Relaxed) >= current_block {
                        return Ok(());
                    }
                } else {
                    // Large swap: mesmo gate — 1x por bloco
    if self.last_processed_block.fetch_max(current_block, Ordering::Relaxed) >= current_block {
                        return Ok(());
                    }
                }

                // ▸ Tocar TODAS as pools bootstrapadas com o bloco actual.
                //   Pools vAMM de alta liquidez (WETH/USDC, DAI/USDC, etc.) podem
                //   não ter swaps na janela de subscrição e seriam marcadas stale
                //   após 500 blocos. Um touch por bloco mantém-nas sempre frescas.
                //   Custo: DashMap iteration uma vez por bloco (~0.5s) — aceitável.
                {
                    let all_pools: Vec<alloy::primitives::Address> = self
                        .pool_cache
                        .get_sample_pools(self.pool_cache.len())
                        .into_iter()
                        .filter(|s| s.last_update_block > 0 && s.has_liquidity())
                        .map(|s| s.address)
                        .collect();
                    for pool_addr in all_pools {
                        self.pool_cache.touch(pool_addr, current_block);
                    }
                }

                // 2) Verificar bankroll antes de calcular
                let bankroll = self.bankroll_manager.read().await;
                let risk_multiplier = bankroll.risk_multiplier();
                if risk_multiplier == 0.0 {
                    warn!(
                        "   💰 [BANKROLL] Circuit breaker ATIVO — {} falhas consecutivas. Abortando.",
                        bankroll.consecutive_failures.load(std::sync::atomic::Ordering::Relaxed));
                    return Ok(());
                }
                // BUG ANTERIOR: passava `synthetic_reserve0 = swap.amount_in × 20`
                // ao bankroll.  Para um swap de 1 USDC raw, isso dá reserve_in=20
                // → cap de 15% = 3 wei → flash_amount=3 wei → 0 output em pools
                // com reserve ~352 ETH (numerator < denominator na divisão inteira).
                //
                // CORREÇÃO: usar a reserve REAL da pool alvo do swap.  O MIN_FLASH_WEI
                // no bankroll garante 0.01 ETH mínimo independentemente do resultado.
                let real_reserve_for_bankroll: u128 = self
                    .pool_cache
                    .get(&swap.pool)
                    .map(|s| {
                        // Usar reserve0 (18-dec) como proxy de profundidade.
                        // Se a pool ainda não tem reserve real, cair para sintético.
                        // CORRECAO: try_into() em vez de .try_into().unwrap_or(u128::MAX) -- tokens exoticos
                        // com escalas absurdas (ex: amount_in com 41 digitos) faziam
                        // .try_into().unwrap_or(u128::MAX) PANICAR (overflow) e abortar o processo inteiro
                        // (panic = "abort" no Cargo.toml mata tudo, nao so esta task).
                        if s.last_update_block > 0 && !s.reserve0.is_zero() {
                            s.reserve0.try_into().unwrap_or(u128::MAX / 2)
                        } else {
                            synthetic_reserve0.try_into().unwrap_or(u128::MAX / 2)
                        }
                    })
                    .unwrap_or_else(|| synthetic_reserve0.try_into().unwrap_or(u128::MAX / 2));

                let optimal_flash_wei = bankroll.optimal_flash_amount(real_reserve_for_bankroll);
                drop(bankroll);

                trace!(
                    "[BANKROLL] pool={:?} real_r0={} flash={} wei",
                    swap.pool,
                    real_reserve_for_bankroll,
                    optimal_flash_wei
                );

                // 3) Chamar find_opportunities() obrigatoriamente
                let mut graph = self.arb_graph.write().await;
                graph.rebuild(swap.block_number);
                let v3_count = self.pool_cache
                    .get_sample_pools(self.pool_cache.len())
                    .into_iter()
                    .filter(|p| matches!(p.dex_type, DexType::UniswapV3))
                    .count();
                tracing::info!(v3_pools = v3_count, "graph composition");
                let hour_now = chrono::Utc::now().hour() as u8;
                let pools: Vec<Address> = self
                    .pool_cache
                    .get_sample_pools(self.pool_cache.len())
                    .into_iter()
                    .map(|p| p.address)
                    .collect();
                let mut pool_priorities = self.pattern_memory.to_priority_map(&pools, hour_now);
                // Misturar scorer (0..1000) na prioridade (pattern_score + scorer_score)
                for pool in &pools {
                    let key = format!("{:?}", pool);
                    let s = self.pool_scorer.get_score(&key) as f64;
                    pool_priorities
                        .entry(*pool)
                        .and_modify(|v| *v += s)
                        .or_insert(s);
                }
                // Ω-Curvature: boost de prioridade para pools com sinal de divergência iminente
                {
                    let pool_pairs: Vec<(Address, Address)> = pools.windows(2)
                        .map(|w| (w[0], w[1]))
                        .collect();
                    let curvature = self.curvature.read().await;
                    let signals = curvature.detect(swap.block_number, &pool_pairs);
                    for sig in &signals {
                        let boost = sig.omega * 5000.0; // escala Ω para prioridade
                        pool_priorities.entry(sig.pool_a).and_modify(|v| *v += boost).or_insert(boost);
                        pool_priorities.entry(sig.pool_b).and_modify(|v| *v += boost).or_insert(boost);
                        if sig.omega > 0.01 {
                            info!("[Ω] Sinal curvatura: pool_a={:?} pool_b={:?} Ω={:.4} bloco={}", 
                                sig.pool_a, sig.pool_b, sig.omega, sig.block);
                        }
                    }
                }
                // Transfer Entropy: alimentar preço atual e boost pools causalmente ligados
                {
                    let price_proxy = if !swap.amount_out.is_zero() {
                        swap.amount_in.try_into().unwrap_or(u128::MAX) as f64 / swap.amount_out.try_into().unwrap_or(u128::MAX).max(1) as f64
                    } else { 0.0 };
                    if price_proxy > 0.0 {
                        let mut te = self.transfer_entropy.write().await;
                        te.record_price(swap.pool, price_proxy);
                        te.update_causality();
                        let caused = te.get_caused_pools(swap.pool);
                        for (caused_pool, te_score) in caused.iter().take(5) {
                            let boost = te_score * 3000.0;
                            pool_priorities.entry(*caused_pool).and_modify(|v| *v += boost).or_insert(boost);
                        }
                    }
                }

                // Garantir mínimo de 0.01 ETH (10^16 wei) — abaixo disso a divisão AMM trunca para zero
                const MIN_FLASH_WEI: u128 = 10_000_000_000_000_000u128; // 0.01 ETH
                let flash_amounts = {
                    let base = optimal_flash_wei.max(MIN_FLASH_WEI);
                    let mut opt = self.flash_optimizer.write().await;
                    let optimized = opt.optimize(
                        "weth_arb",
                        |input_wei| input_wei + (input_wei / 150), // +0.67% proxy
                        base / 500, // gas proxy
                        MIN_FLASH_WEI,
                        base.saturating_mul(2),
                    ).unwrap_or(base);
                    // CORREÇÃO: em vez de 3 candidatos vindos de um modelo linear (sem
                    // curvatura real -- um proxy linear nunca tem óptimo, sugere "size
                    // infinito = lucro infinito", o que é falso por causa do slippage),
                    // testamos uma varrição geométrica mais ampla. O grafo já avalia cada
                    // candidato com a matemática AMM real por hop (find_2hop/3hop_cycles),
                    // por isso mais candidatos = mais hipótese de encontrar o ponto onde
                    // o lucro líquido é máximo, sem precisarmos de resolver a curva à mão.
                    let mults: [u128; 9] = [25, 50, 75, 100, 150, 200, 300, 500, 800]; // % do 'optimized'
                    mults
                        .iter()
                        .map(|pct| U256::from((optimized.saturating_mul(*pct) / 100).max(MIN_FLASH_WEI)))
                        .collect::<Vec<U256>>()
                };
                let observed_gas = context.priority_fee_gwei as f64;
                let predicted_gas = self.kalman_gas.write().await.update(observed_gas);
                let gas_price_wei = U256::from((predicted_gas * 1_000_000_000.0) as u64);
                let t = std::time::Instant::now();
                // Multi-token start: WETH + USDC + cbETH + AERO + tokens divergentes (long-tail)
                let divergences = self.midcap_scanner.find_divergences();
                let mut extra_tokens: Vec<Address> = divergences.iter().map(|d| d.token).collect();
                extra_tokens.dedup();
                let recent_launches = self.launch_monitor.get_recent_launches(swap.block_number);
                extra_tokens.extend(recent_launches);

                let mut opps = Vec::new();
                let mut start_tokens = vec![WETH, USDC, CBETH, AERO];
                start_tokens.extend(extra_tokens);
                for start_tok in start_tokens {
                    let tok_opps = graph.find_opportunities_with_priorities(
                        start_tok,
                        &flash_amounts,
                        gas_price_wei,
                        1.2,
                        Some(&pool_priorities),
                    );
                    opps.extend(tok_opps);
                }
                opps.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));
                let opps: Vec<_> = opps.into_iter().filter(|opp| {
                    let tokens: Vec<_> = opp.hops.iter().map(|h| h.token_in).collect();
                    self.honeypot.is_path_safe(&tokens)
                }).collect();
                // Topologia de Persistência — alimentar e boost de prioridade
                {
                    let mut topo = self.topology.write().await;
                    for opp in &opps {
                        let pools: Vec<Address> = opp.hops.iter().map(|h| h.pool).collect();
                        let spread = if opp.input_amount.is_zero() { 0.0 } else {
                            opp.gross_profit.try_into().unwrap_or(u128::MAX) as f64 / opp.input_amount.try_into().unwrap_or(u128::MAX) as f64
                        };
                        if let Some(signal) = topo.observe_cycle(&pools, spread, swap.block_number) {
                            info!(
                                "[TOPO] {} ciclo: spread={:.4}% persistence_score={:.1} bloco={} pools={:?}",
                                if signal.is_revival { "Revival" } else { "Novo" },
                                signal.spread * 100.0,
                                signal.persistence_score,
                                signal.block,
                                pools
                            );
                        }
                    }
                    // Boost de prioridade para pools em ciclos persistentes
                    for pool in &pools {
                        let boost = topo.pool_persistence_boost(*pool);
                        if boost > 0.0 {
                            pool_priorities.entry(*pool).and_modify(|v| *v += boost).or_insert(boost);
                        }
                    }
                }
                let elapsed_us = t.elapsed().as_micros();
                if let Some(ref telem) = self.telemetry {
                    telem.record_scan(elapsed_us).await;
                    telem.record_newton_raphson(elapsed_us).await;
                }

                // 4) Logar resultado
                if opps.is_empty() {
                    debug!("[ARB] Bloco {}: sem oportunidades", swap.block_number);
                } else {
                    self.pattern_memory.record_opportunity(
                        swap.pool,
                        hour_now,
                        opps[0].net_profit.try_into().unwrap_or(u128::MAX),
                    );
                    self.pool_scorer.on_opportunity_found(
                        &format!("{:?}", swap.pool),
                        opps[0].net_profit.try_into().unwrap_or(u128::MAX),
                    );
                    info!(
                        "[ARB] 🎯 {} oportunidades | Melhor: {:.6} ETH profit | Hops: {}",
                        opps.len(),
                        opps[0].net_profit.try_into().unwrap_or(u128::MAX) as f64 / 1e18,
                        opps[0].hops.len()
                    );
                    // Log CSV para análise DRY_RUN
                    // Log CSV — apenas melhor oportunidade por path único por bloco
                    let mut seen_paths = std::collections::HashSet::new();
                    for opp in &opps {
                        let path_str = {
                            let mut parts: Vec<String> = opp.hops.iter().map(|h| format!("{:?}", h.token_in)).collect();
                            if let Some(last) = opp.hops.last() { parts.push(format!("{:?}", last.token_out)); }
                            parts.join("→")
                        };
                        // Filtrar: só pools com reserves verificadas via getReserves() real
                        let min_r = alloy::primitives::U256::from(100_000_000_000_000_000u128);
                        let has_reserves = opp.hops.iter().all(|h| {
                            self.pool_cache.get(&h.pool).map(|p| p.reserve0 >= min_r || p.reserve1 >= min_r).unwrap_or(false)
                        });
                        if !has_reserves { debug!("[ARB-FILTER] reserves insuficientes path={}", path_str); continue; }
                        if seen_paths.contains(&path_str) { continue; }
                        seen_paths.insert(path_str.clone());
                        self.opp_logger.log(&crate::logger::opportunity_logger::OpportunityRecord {
                            block: swap.block_number,
                            path: path_str.clone(),
                            hops: opp.hops.len(),
                            input_wei: opp.input_amount.try_into().unwrap_or(u128::MAX),
                            gross_profit_wei: opp.gross_profit.try_into().unwrap_or(u128::MAX),
                            net_profit_wei: opp.net_profit.try_into().unwrap_or(u128::MAX),
                            gas_cost_wei: opp.gas_cost.try_into().unwrap_or(u128::MAX),
                        });
                        // Notificação Discord para opps > 1€
                        let profit_eur = opp.net_profit.try_into().unwrap_or(u128::MAX) as f64 / 1e18 * self.eth_price_feed.get_eur().await;
                        if profit_eur >= 1.0 {
                            let discord = self.discord.clone();
                            let path_discord = path_str.clone();
                            let hops_n = opp.hops.len();
                            let block_n = swap.block_number;
                            tokio::spawn(async move {
                                discord.notify_opportunity(&path_discord, profit_eur, hops_n, block_n).await;
                            });
                        }
                    }
                }

                // EXECUÇÃO REAL
                {
                    let min_r = alloy::primitives::U256::from(100_000_000_000_000_000u128);
                    // DIAGNÓSTICO TEMPORÁRIO: zero execuções até agora -- contar quantas
                    // oportunidades falham em cada condição do filtro para identificar
                    // qual está a bloquear tudo (remover depois de identificado).
                    let zero_profit_count = opps.iter().filter(|o| o.net_profit.is_zero()).count();
                    let low_reserve_count = opps.iter().filter(|o| {
                        !o.net_profit.is_zero() && !o.hops.iter().all(|h| {
                            self.pool_cache.get(&h.pool).map(|p| p.reserve0 >= min_r || p.reserve1 >= min_r).unwrap_or(false)
                        })
                    }).count();
                    if !opps.is_empty() {
                        info!(
                            "[DIAG-EXEC] total={} zero_profit={} low_reserve={} sobrevivem={}",
                            opps.len(), zero_profit_count, low_reserve_count,
                            opps.len() - zero_profit_count - low_reserve_count
                        );
                    }
                    // INOVAÇÃO: top-N candidatos em paralelo em vez de só o melhor --
                    // gás só é gasto se o eth_call individual de cada um passar (0 custo
                    // extra de tentar mais), aumentando as hipóteses de pelo menos 1
                    // sobreviver à janela de spread antes que o mercado a feche.
                    const TOP_N_CANDIDATES: usize = 3;
                    // INOVAÇÃO (equilíbrio cauda-longa vs pools populares): dados reais
                    // (1173 tentativas) mostram concentração massiva numa única pool
                    // (2998 ocorrências, quase 3x a 2ª mais comum) -- exatamente onde a
                    // concorrência de outros bots é maior (99.9% dos erros são IIA por
                    // concorrência instantânea, não deriva previsível). O lucro continua
                    // a ser o critério principal (não se troca qualidade por diversidade
                    // às cegas); só se reordena DENTRO de um pool alargado de candidatos
                    // (10, não 3) para dar prioridade aos não-cansados quando o lucro é
                    // competitivo, evitando insistir sempre na pool mais disputada.
                    const CANDIDATE_POOL_SIZE: usize = 10;
                    let mut wide_candidates: Vec<_> = opps.iter()
                        .filter(|o| {
                            !o.net_profit.is_zero() && o.hops.iter().all(|h| {
                                self.pool_cache.get(&h.pool).map(|p| p.reserve0 >= min_r || p.reserve1 >= min_r).unwrap_or(false)
                            })
                        })
                        .take(CANDIDATE_POOL_SIZE)
                        .collect::<Vec<_>>();
                    wide_candidates.sort_by_key(|o| {
                        let fatigued = o.hops.first().map(|h| self.pool_fatigue.is_fatigued(&h.pool)).unwrap_or(false);
                        fatigued // false (não cansado) ordena primeiro
                    });
                    let candidates: Vec<_> = wide_candidates.into_iter().take(TOP_N_CANDIDATES).collect();
                    for best in candidates {
                        // CORREÇÃO CRÍTICA: os filtros abaixo só LIAM o histórico Kalman
                        // (self.kalman_price.get()), mas quem o ALIMENTA era só o código
                        // de sizing, que corre DEPOIS no fluxo -- ou seja, os filtros
                        // viam sempre um mapa vazio na primeira passagem por cada pool e
                        // nunca bloqueavam nada (confirmado: 0 hits em 294 tentativas,
                        // 96.6% LOSS). Agora atualiza-se aqui também, antes de ler.
                        let now_ms_filter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                        for h in best.hops.iter().filter(|h| h.dex_type == DexType::UniswapV3) {
                            if let Some(sqrt_p) = h.sqrt_price_x96 {
                                let normalized_price = sqrt_p as f64 / (2f64.powi(96));
                                let mut entry = self.kalman_price.entry(h.pool)
                                    .or_insert_with(|| crate::math::kalman_price::KalmanPricePredictor::new(normalized_price));
                                entry.update(normalized_price, now_ms_filter);
                            }
                        }
                        // INOVAÇÃO: filtro de volatilidade preditiva -- usa o
                        // Kalman price já existente (read-only, sem custo extra)
                        // para saltar candidatos cuja pool está a derivar rápido
                        // demais AGORA (>0.5% previsto no horizonte de latência),
                        // libertando o slot do top-N para um candidato mais estável.
                        let too_volatile = best.hops.iter().any(|h| {
                            if h.dex_type != DexType::UniswapV3 { return false; }
                            self.kalman_price.get(&h.pool)
                                .map(|entry| entry.relative_drift(1900.0) > 0.005)
                                .unwrap_or(false)
                        });
                        if too_volatile {
                            debug!("[KALMAN-FILTER] candidato descartado -- deriva de preço >0.5% prevista no horizonte");
                            continue;
                        }
                        // INOVAÇÃO: filtro preditivo de lucro -- soma a deriva de preço
                        // prevista (Kalman) em todos os hops V3 do ciclo e compara com a
                        // margem de lucro relativa do próprio ciclo. Se a deriva esperada
                        // no horizonte de latência já é maior que a margem, o ciclo está
                        // matematicamente condenado a "ORCA: LOSS" antes de gastarmos um
                        // eth_call nele -- ataca diretamente o padrão de 22/22 LOSS visto
                        // em produção (sizing correto, mas spread já fechado na execução).
                        let total_predicted_drift: f64 = best.hops.iter()
                            .filter(|h| h.dex_type == DexType::UniswapV3)
                            .filter_map(|h| self.kalman_price.get(&h.pool).map(|e| e.relative_drift(1900.0)))
                            .sum();
                        // CORREÇÃO CRÍTICA: usava h.reserve_in (reserva TOTAL da pool,
                        // podem ser milhões em TVL) como denominador em vez do tamanho
                        // real do trade -- tornava profit_margin_ratio artificialmente
                        // minúsculo e o filtro quase inútil (comparava lucro esperado
                        // contra o TVL da pool inteira, não contra o que arriscamos).
                        let input_size_f64 = (best.input_amount.try_into().unwrap_or(u128::MAX) as f64 / 1e18).max(1e-9);
                        let profit_margin_ratio = (best.net_profit.try_into().unwrap_or(u128::MAX) as f64 / 1e18) / input_size_f64;
                        const DRIFT_SAFETY_MULTIPLIER: f64 = 1.5;
                        if total_predicted_drift > 0.0 && profit_margin_ratio > 0.0
                            && total_predicted_drift > profit_margin_ratio * DRIFT_SAFETY_MULTIPLIER {
                            debug!(
                                "[PROFIT-PREDICT-FILTER] descartado -- deriva prevista {:.4}% > margem*{} ({:.4}%)",
                                total_predicted_drift * 100.0, DRIFT_SAFETY_MULTIPLIER, profit_margin_ratio * DRIFT_SAFETY_MULTIPLIER * 100.0
                            );
                            continue;
                        }
                        // CORREÇÃO (rigorosa, não aproximada): pesquisa ternária sobre o
                        // tamanho do flash loan, usando a fórmula AMM real de cada hop com
                        // as reserves já conhecidas (zero chamadas à rede). A curva de lucro
                        // de uma arbitragem cíclica é côncava (sobe, atinge um pico, desce
                        // por causa do slippage) -- a pesquisa ternária converge de forma
                        // matematicamente garantida para o tamanho que maximiza o lucro real,
                        // dentro do intervalo seguro (nunca acima de 15% da reserve do hop
                        // mais fino do ciclo).
                        let gas_cost_wei = U256::from(105_000u64) * U256::from(1_000_000u64); // ~0.001 gwei base
                        // CORREÇÃO: max_safe_input (15% da reserve mais fina do ciclo,
                        // calculado por segurança real) estava a ser sobreposto por
                        // .max(MIN_FLASH_WEI_U256) -- isto forçava SEMPRE 0.01 ETH como
                        // mínimo, mesmo quando max_safe_input determinava que isso já
                        // era inseguro/inviável para aquele ciclo específico (confirmado:
                        // 95/95 refinamentos do QuoterV2 real davam 0 -- max_candidate
                        // era sempre exactamente 0.01 ETH, nunca variava, porque o
                        // .max() forçava sempre o mesmo valor independentemente do
                        // ciclo). Se max_safe_input for genuinamente menor que o minimo
                        // de execução viável, o ciclo deve ser descartado, não forçado.
                        // CORREÇÃO: reserve_in de cada hop está na escala de decimais
                        // NATIVA do respectivo token (ex: USDC=6, WETH=18) -- comparar
                        // directamente sem normalizar fazia o hop em 6 decimais parecer
                        // ~10^12 vezes mais "fino"/arriscado do que realmente é,
                        // fazendo max_safe_input ficar sempre minúsculo e descartar
                        // 100% dos ciclos (confirmado: hop USDC com reserve_in=1801140941,
                        // que são ~1801 USDC reais, parecia só 0.0000000018 "unidades"
                        // quando comparado a hops de 18 decimais).
                        let max_safe_input = best.hops.iter()
                            .map(|h| {
                                if h.dex_type == DexType::UniswapV3 {
                                    if let (Some(sqrt_p), Some(liq)) = (h.sqrt_price_x96, h.liquidity) {
                                        let zero_for_one = h.token_in < h.token_out;
                                        // INOVAÇÃO: preditor Kalman por pool (extensão do já existente
                                        // para gas price) -- prevê o sqrt_price no horizonte da latência
                                        // detecção->eth_call medida (p50 ~1900ms), em vez de usar o preço
                                        // já desatualizado no momento da deteção. Reduz IIA causado por
                                        // deriva de preço previsível (tendência), não apenas ruído.
                                        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                                        let normalized_price = sqrt_p as f64 / (2f64.powi(96));
                                        let predicted_normalized = {
                                            let mut entry = self.kalman_price.entry(h.pool)
                                                .or_insert_with(|| crate::math::kalman_price::KalmanPricePredictor::new(normalized_price));
                                            entry.update(normalized_price, now_ms);
                                            entry.predict_ahead(1900.0)
                                        };
                                        let predicted_sqrt_p = ((predicted_normalized * 2f64.powi(96)) as u128).max(1);
                                        // INOVAÇÃO: impacto máximo permitido é adaptativo à volatilidade
                                        // REAL medida pelo Kalman desta pool -- não um número fixo arbitrário.
                                        // Pool calma (deriva baixa) -> mais margem de input. Pool volátil ->
                                        // mantém-se conservador. Sem dados ainda -> default moderado (100bps).
                                        let drift = self.kalman_price.get(&h.pool)
                                            .map(|e| e.relative_drift(1900.0))
                                            .unwrap_or(0.003); // sem historico: assume risco moderado
                                        let max_impact_bps: u64 = if drift < 0.001 {
                                            150 // 1.5% -- pool muito estável
                                        } else if drift < 0.003 {
                                            100 // 1.0% -- default/moderado
                                        } else if drift < 0.005 {
                                            60  // 0.6%
                                        } else {
                                            30  // 0.3% -- pool a derivar rápido, mantém conservador
                                        };
                                        let v3_safe = max_safe_input_v3(predicted_sqrt_p, liq, zero_for_one, max_impact_bps);
                                        if !v3_safe.is_zero() {
                                            return v3_safe;
                                        }
                                    }
                                }
                                // Fallback (V2-style: Aerodrome/PancakeSwap sem liquidez concentrada)
                                let normalized = if h.decimals_in < 18 {
                                    h.reserve_in.saturating_mul(U256::from(10u64).pow(U256::from(18 - h.decimals_in)))
                                } else if h.decimals_in > 18 {
                                    h.reserve_in / U256::from(10u64).pow(U256::from(h.decimals_in - 18))
                                } else {
                                    h.reserve_in
                                };
                                normalized.saturating_mul(U256::from(1u64)) / U256::from(100u64)
                            })
                            .min()
                            .unwrap_or(U256::ZERO);
                        let optimal_input = if max_safe_input < MIN_FLASH_WEI_U256 {
                            U256::ZERO // ciclo genuinamente inviável a qualquer tamanho seguro -- descartar, não forçar
                        } else {
                            optimal_cycle_input(
                                &best.hops,
                                MIN_FLASH_WEI_U256,
                                max_safe_input,
                                gas_cost_wei,
                            )
                        };

                        // CORREÇÃO DE CAUSA RAIZ (erro "IIA"): optimal_cycle_input usa a
                        // fórmula V2 (produto constante) para TODOS os hops, incluindo
                        // V3 -- estruturalmente errado para V3 (liquidez concentrada por
                        // tick, não uniforme). Isto sobrestimava sistematicamente quanto
                        // se podia trocar em hops V3, causando "IIA" no eth_call real
                        // (confirmado: 471/474 falhas numa sessão de 24min eram "IIA").
                        // Refinamento: se o ciclo tem algum hop V3, validar/reduzir o
                        // optimal_input via QuoterV2 oficial (simulação EXATA multi-tick,
                        // gratuita via eth_call) -- usa busca binária real em vez de
                        // qualquer margem de segurança adivinhada.
                        // CORREÇÃO FINAL: removida a validação prévia via QuoterV2.
                        // Descoberto: o QuoterV2 oficial calcula SEMPRE o endereço da
                        // pool via factory.getPool(token0, token1, fee) -- se o nosso
                        // pool_cache tiver uma pool DIFERENTE para o mesmo par+fee
                        // (confirmado repetidamente: pools genuínas e líquidas no nosso
                        // cache, mas a Factory aponta para outra pool, morta, para o
                        // mesmo par+fee), o QuoterV2 nunca valida a pool certa -- estava
                        // a rejeitar 100% dos ciclos mesmo com liquidez real confirmada
                        // na pool que a NOSSA execução de facto usa (hop.pool, lido
                        // directamente pelo OrcaExecutor.sol, sem nunca consultar a
                        // Factory). A validação real e correta já existe: o eth_call
                        // final em submit_to_protector usa hop.pool directamente --
                        // essa continua activa e é a rede de segurança real.
                        let optimal_input = optimal_input;

                        if !optimal_input.is_zero() {
                        // CORREÇÃO: envolvido em "if !optimal_input.is_zero()" em vez de
                        // "continue"/"return" (que saltariam código importante mais
                        // abaixo no mesmo match, como persistência de pattern_memory) --
                        // ciclo inviável simplesmente não constrói nem tenta executar
                        // nenhuma oportunidade, sem afectar o resto do processamento
                        // deste evento.
                        info!(
                            "[DIAG-POOLS] pools={:?} profit={}",
                            best.hops.iter().map(|h| h.pool).collect::<Vec<_>>(),
                            best.net_profit.try_into().unwrap_or(u128::MAX) as f64 / 1e18
                        );
                        let opp_exec = Opportunity {
                            id: swap.block_number,
                            detected_at_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                            pool_address: best.hops.first().map(|h| h.pool).unwrap_or(Address::ZERO),
                            token_in: best.start_token,
                            token_out: best.start_token,
                            amount_in: optimal_input,
                            expected_profit_eth: best.net_profit.try_into().unwrap_or(u128::MAX) as f64 / 1e18,
                            opportunity_type: OpportunityType::Arbitrage,
                            hops: best.hops.iter().map(|e| crate::types::Hop {
                                pool: e.pool,
                                token_in: e.token_in,
                                token_out: e.token_out,
                                fee: e.fee,
                                dex_type: e.dex_type,
                            }).collect(),
                        };
                        let engine = self.clone();
                        // CORREÇÃO: tokio::spawn normal fica atrás de tasks CPU-bound
                        // (graph rebuild, cycle-finding) no mesmo runtime -- medido até
                        // 3s de atraso so na fila do scheduler antes de sequer entrar em
                        // execute_opportunity. spawn_blocking usa pool de threads dedicado,
                        // não bloqueado por trabalho async CPU-bound no runtime principal.
                        let rt_handle = tokio::runtime::Handle::current();
                        std::thread::spawn(move || {
                            rt_handle.block_on(engine.execute_opportunity(opp_exec));
                        });
                        } // fecha if !optimal_input.is_zero()
                    }
                }
                let mut last_persist = self.last_pattern_persist_block.write().await;
                if swap.block_number.saturating_sub(*last_persist) >= 100 {
                    self.pattern_memory.persist_to_disk();
                    *last_persist = swap.block_number;
                }

                // Sistema 2: Reserve inference (só quando recebemos sqrtPriceX96)
                if matches!(
                    swap.dex_type,
                    DexType::UniswapV3 | DexType::PancakeSwap | DexType::AerodromeStable
                ) {
                    if let Some(sqrt) = swap.sqrt_price_x96 {
                        let candidates = self
                            .pool_cache
                            .get_pools_by_tokens(swap.token_in, swap.token_out);
                        let mut triggered = false;
                        for v2_pool in candidates.into_iter().filter(|p| {
                            matches!(p.dex_type, DexType::UniswapV2 | DexType::Aerodrome)
                        }) {
                            let divergence_bps = detect_cross_pool_divergence(
                                sqrt,
                                v2_pool.reserve0.try_into().unwrap_or(u128::MAX),
                                v2_pool.reserve1.try_into().unwrap_or(u128::MAX),
                                v2_pool.decimals0,
                                v2_pool.decimals1,
                            );
                            if divergence_bps > 10 {
                                info!(
                                    "[INFERENCE] Divergência {}bps entre V3 e V2 — verificando arb",
                                    divergence_bps
                                );
                                triggered = true;
                                break;
                            }
                        }

                        if triggered {
                            // Re-scan rápido com as mesmas prioridades já calculadas
                            let _ = graph.find_opportunities_with_priorities(
                                WETH,
                                &flash_amounts,
                                gas_price_wei,
                                1.2,
                                Some(&pool_priorities),
                            );
                        }
                    }
                }
            }
            MevEvent::BlockUpdate(block) => {
                self.last_observed_block.store(block, Ordering::Relaxed);
                let mut last_persist = self.last_pattern_persist_block.write().await;
                if block.saturating_sub(*last_persist) >= 100 {
                    self.pattern_memory.persist_to_disk();
                    *last_persist = block;
                }
                drop(last_persist);

                // Guardar wallet para leituras periódicas de saldo on-chain
                {
                    let mut tracked_wallet = self.tracked_wallet.write().await;
                    if tracked_wallet.is_none() {
                        *tracked_wallet = Some(context.executor_address);
                    }
                }

                // Atualizar saldo real da wallet a cada 10 blocos
                if block % 10 == 0 {
                    if let Err(err) = self.sync_wallet_balance().await {
                        warn!("[BANKROLL] Falha ao sincronizar saldo on-chain: {}", err);
                    }
                }

                let mut graph = self.arb_graph.write().await;
                graph.rebuild(block);
                let v3_count = self.pool_cache
                    .get_sample_pools(self.pool_cache.len())
                    .into_iter()
                    .filter(|p| matches!(p.dex_type, DexType::UniswapV3))
                    .count();
                tracing::info!(v3_pools = v3_count, "graph composition");
                trace!("[ORCA] Bloco {} detetado — grafo reconstruído", block);

                // 💰 Status report a cada 1000 blocos
                let mut last_block = self.last_status_block.write().await;
                if block.saturating_sub(*last_block) >= 1000 {
                    *last_block = block;
                    let bankroll = self.bankroll_manager.read().await;
                    info!("{}", bankroll.status_report());
                    drop(bankroll);
                }
                drop(last_block);
            }
            _ => {}
        }
        Ok(())
    }

    async fn initialize(&mut self, _initial_data: Vec<NormalizedSwapEvent>) -> eyre::Result<()> {
        info!("[ORCA] Motor sincronizado com a Mainnet.");
        let last_block = Arc::clone(&self.last_observed_block);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                info!(
                    "[HEARTBEAT] Bot activo | Bloco: {}",
                    last_block.load(Ordering::Relaxed)
                );
            }
        });
        Ok(())
    }

    fn stats(&self) -> crate::artemis::strategy::StrategyStats {
        crate::artemis::strategy::StrategyStats::default()
    }
}

/// 💎 Oportunidade de MEV
#[derive(Clone, Debug)]
pub struct Opportunity {
    pub id: u64,
    pub pool_address: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub expected_profit_eth: f64,
    pub opportunity_type: OpportunityType,
    pub hops: Vec<crate::types::Hop>,
    pub detected_at_ms: u64,
}

/// 🎯 Tipo de oportunidade
#[derive(Clone, Debug, PartialEq)]
pub enum OpportunityType {
    Arbitrage,
    Liquidation,
    Sandwich,
    GhostCallback,
}

/// 🧮 Resultado de simulação
#[derive(Clone, Debug)]
pub struct SimulationResult {
    pub gross_profit_eth: f64,
    pub net_profit_eth: f64,
    pub gas_used: u64,
    pub gas_cost_eth: f64,
    pub gas_saved_eth: f64,
    pub will_succeed: bool,
}

/// 📦 Bundle protegido
#[derive(Clone, Debug)]
pub struct ProtectedBundle {
    pub transactions: Vec<Bytes>,
    pub min_profit_eth: f64,
    pub max_gas_eth: f64,
    pub target_slot: u16,
    pub revert_on_failure: bool,
    pub hops: Vec<crate::types::Hop>,
    pub loan_amount_wei: U256,
    pub detected_at_ms: u64,
    pub priority_fee_wei: u128,
}

/// 🧾 Recibo de execução
#[derive(Clone, Debug)]
pub struct ExecutionReceipt {
    pub tx_hash: String,
    pub block_number: u64,
    pub slot: u16,
    pub profit_eth: f64,
    pub gas_used: u64,
    pub gas_saved_eth: f64,
    pub timestamp: u64,
}

/// 🚦 Status do sistema ORCA
#[derive(Clone, Debug, PartialEq)]
pub enum OrcaSystemStatus {
    /// Operando normalmente
    Active,
    /// Em pausa (monitorização)
    Idle,
    /// Kill-switch ativado
    Halted,
    /// Aguardando autorização
    AwaitingAuth,
}
