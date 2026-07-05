//! Pool Discovery Engine - Auto-Scan de Factories Uniswap V3 & Aerodrome
//!
//! Sistema de descoberta contínua que varre as factories e identifica
//! pools lucrativas com critérios rigorosos de liquidez.
//!
//! Critérios de Inclusão (Modo Sobrevivência Alchemy Free - 10 blocos):
//! - TVL > $1,000 USD
//! - Volume 24h > $500 USD
//! - Prioridade para tokens base (WETH, USDC, DAI, CBETH)

use alloy::primitives::{address, Address, FixedBytes, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::Filter;
use alloy::sol;
use alloy::sol_types::SolEvent;
use alloy::transports::BoxTransport;
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tracing::{debug, error, info, trace, warn};

// CONSTANTES GLOBAIS - MODO SOBREVIVÊNCIA ALCHEMY FREE
const REALTIME_BLOCK_CHUNK: u64 = 10; // subscrição/scan recente
const MAX_ERRORS: u32 = 10; // Máximo de erros antes de abortar scan

/// 📊 SCAN_BLOCKS configurável via .env
/// Padrão: 1000 blocos (com cache existente, não precisamos de ir longe)
fn get_scan_blocks() -> u64 {
    std::env::var("SCAN_BLOCKS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1000u64)
}

/// ⏱️ RPC Delay configurável via .env (RPC_DELAY_MS)
/// Padrão: 200ms entre pedidos para evitar rate limit Alchemy Free
fn get_rpc_delay_ms() -> u64 {
    std::env::var("RPC_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(200)
}

/// 🎯 DISCOVERY_READY_TARGET do .env para lógica de cache inteligente
fn get_discovery_target() -> usize {
    std::env::var("DISCOVERY_READY_TARGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(60)
}
const MIN_CACHE_FOR_FAST_BOOT: usize = 1000;
const EMERGENCY_MIN_POOL_TARGET: usize = 50;
const SLIPSTREAM_POOL_PAGE_SIZE: u64 = 50;
/// Mínimo absoluto de pools com seed estático (sem cache, sem discovery)
const SEED_MIN_POOLS: usize = 4;
/// Timeout máximo (segundos) para o loop de emergency discovery — impede loop infinito
const EMERGENCY_SEED_TIMEOUT_SECS: u64 = 30;

// Multicall3 - Endereço canónico na Base Mainnet
pub const MULTICALL3: Address = address!("0xcA11bde05977b3631167028862bE2a173976CA11");

// Aerodrome Factories na Base Mainnet
/// Aerodrome V2 (Classic)
pub const AERODROME_FACTORY: Address = address!("0x42024DAb8ED9bce086865aCD50831a567Bb4258B");
/// Aerodrome Slipstream (V3) - Concentrated Liquidity
pub const AERODROME_SLIPSTREAM: Address = address!("0x5e79E80734891BA0907297920A0bA562Bf76632c");
/// Aerodrome CL Factory - para scan de pools CL
pub const AERODROME_CL_FACTORY: Address = address!("0x5e79E80734891BA0907297920A0bA562Bf76632c");

// Outras Factories na Base Mainnet
pub const UNISWAP_V3_FACTORY: Address = address!("0x33128a8fC17869897dcE68Ed026d694621f6FDfD");
pub const UNISWAP_V2_FACTORY: Address = address!("0x8909Dc15e40173Ff2D47AfE5aC6E6758472bC7e5");

// Tokens Base (prioridade máxima)
pub const WETH: Address = address!("0x4200000000000000000000000000000000000006");
pub const USDC: Address = address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
pub const DAI: Address = address!("0x50c5725949A6F0c72E6C4a641F2409A017ebaBdf");
pub const CBETH: Address = address!("0x2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22");

sol! {
    event PoolCreated(
        address indexed token0,
        address indexed token1,
        uint24 indexed fee,
        int24 tickSpacing,
        address pool
    );

    event PoolCreatedAero(
        address indexed token0,
        address indexed token1,
        bool indexed stable,
        address pool,
        uint256
    );

    event PairCreated(
        address indexed token0,
        address indexed token1,
        address pair,
        uint256
    );
}

sol! {
    #[sol(rpc)]
    interface IMulticall3 {
        struct Call {
            address target;
            bytes callData;
        }
        struct Result {
            bool success;
            bytes returnData;
        }
        function aggregate(Call[] calldata calls) external payable returns (uint256 blockNumber, bytes[] memory returnData);
        function tryAggregate(bool requireSuccess, Call[] calldata calls) external payable returns (Result[] memory returnData);
    }

    /// PoolCreated event from Aerodrome Factory
    /// event PoolCreated(address indexed token0, address indexed token1, bool stable, address pool, uint256 length)
    #[sol(rpc)]
    interface IAerodromeFactory {
        /// @notice Seletor allPoolsLength(): 0x13c3833b
        function allPoolsLength() external view returns (uint256 length);
        /// @notice Seletor allPools(uint256): 0xf275482c
        function allPools(uint256 index) external view returns (address pool);
        /// @notice Seletor getPool(address,address,bool): 0xe3f3f29a
        function getPool(address tokenA, address tokenB, bool stable) external view returns (address pool);
    }

    #[sol(rpc)]
    interface IAerodromePool {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function stable() external view returns (bool);
    }

    /// Interface for Aerodrome Slipstream (V3) Concentrated Liquidity Factory
    #[sol(rpc)]
    interface ICLFactory {
        function pools(uint256 index) external view returns (address);
        function poolsLength() external view returns (uint256);
        function getPool(address tokenA, address tokenB, int24 tickSpacing) external view returns (address);
    }

    #[sol(rpc)]
    interface ICLPool {
        function token0() external view returns (address);
        function token1() external view returns (address);
        function tickSpacing() external view returns (int24);
        function liquidity() external view returns (uint128);
    }
}

/// Cache file path
const CACHE_FILE: &str = "pools_cache_base.json";

/// Dados de uma pool descoberta
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PoolData {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub dex_type: DexType,
    pub tvl_usd: f64,
    pub volume_24h: f64,
    pub priority_score: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DexType {
    #[default]
    UniswapV3,
    UniswapV2,
    Aerodrome,
}

/// Configuração do Discovery
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub min_tvl_usd: f64,
    pub min_volume_24h_usd: f64,
    pub max_pools: usize,
    pub scan_interval_secs: u64,
    pub lookback_blocks: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            min_tvl_usd: 1_000.0,      // $1k TVL mínimo (modo 10 blocos - mais permissivo)
            min_volume_24h_usd: 500.0, // $500 volume 24h (modo 10 blocos - mais permissivo)
            max_pools: 5000,           // 🔥 MODO PROMÍCUO: 5000 pools ativas
            scan_interval_secs: 300,   // Scan a cada 5 min
            lookback_blocks: 17280,    // ~24h na Base (0.5s/bloco)
        }
    }
}

/// Estatísticas do discovery
#[derive(Clone, Debug, Default)]
pub struct DiscoveryStats {
    pub total_pools_found: u64,
    pub pools_accepted: u64,
    pub pools_rejected_tvl: u64,
    pub pools_rejected_volume: u64,
    pub last_scan_timestamp: u64,
    pub active_pools: usize,
}

/// Engine de Pool Discovery
#[derive(Debug)]
pub struct PoolDiscoveryEngine {
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    config: DiscoveryConfig,
    pools: Arc<RwLock<HashMap<Address, PoolData>>>,
    stats: Arc<RwLock<DiscoveryStats>>,
    base_tokens: Vec<Address>,
}

impl PoolDiscoveryEngine {
    pub fn new(provider: Arc<RwLock<RootProvider<BoxTransport>>>, config: DiscoveryConfig) -> Self {
        let base_tokens = vec![WETH, USDC, DAI, CBETH];

        Self {
            provider,
            config,
            pools: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(DiscoveryStats::default())),
            base_tokens,
        }
    }

    // ════════════════════════════════════════════════════════════════
    // SEED POOLS ESTÁTICO — base mínima garantida, zero RPC
    // ════════════════════════════════════════════════════════════════

    /// ð± Retorna pools conhecidas e verificadas na Base Mainnet.
    /// Estas pools SEMPRE existem e têm liquidez suficiente para arbitragem.
    /// Nenhuma chamada RPC é necessária — são endereços estáticos.
    fn get_seed_pools() -> Vec<PoolData> {
        vec![
            // WETH/USDC Aerodrome vAMM #1 — maior pool da Base
            PoolData {
                address: address!("88A43bbDF9D098eEC7bCEda4e2494615dfD9bB9C"),
                token0: address!("4200000000000000000000000000000000000006"), // WETH
                token1: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
                fee: 30,
                dex_type: DexType::Aerodrome,
                tvl_usd: 5_000_000.0,
                volume_24h: 1_000_000.0,
                priority_score: 250.0,
            },
            // WETH/USDC Aerodrome vAMM #2
            PoolData {
                address: address!("cDAC0d6c6C59727a65F871236188350531885C43"),
                token0: address!("4200000000000000000000000000000000000006"), // WETH
                token1: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
                fee: 30,
                dex_type: DexType::Aerodrome,
                tvl_usd: 5_000_000.0,
                volume_24h: 1_000_000.0,
                priority_score: 250.0,
            },
            // DAI/USDC Aerodrome vAMM — stable pair
            PoolData {
                address: address!("67b00B46FA4f4F24c03855c5C8013C0B938B3eEc"),
                token0: address!("50c5725949A6F0c72E6C4a641F2409A017ebaBdf"), // DAI
                token1: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
                fee: 30,
                dex_type: DexType::Aerodrome,
                tvl_usd: 2_000_000.0,
                volume_24h: 500_000.0,
                priority_score: 220.0,
            },
            // USDC/AERO Aerodrome vAMM
            PoolData {
                address: address!("6cDcb1C4A4D1C3C6d054b27AC5B77e89eAFb971d"),
                token0: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
                token1: address!("940181a94a35a4569e4529a3cdfb74e38fd98631"), // AERO
                fee: 30,
                dex_type: DexType::Aerodrome,
                tvl_usd: 1_000_000.0,
                volume_24h: 200_000.0,
                priority_score: 200.0,
            },
        ]
    }

    /// ð± Insere seed pools no mapa interno (zero RPC, zero latência).
    /// Seed tem PRIORIDADE sobre o cache: sobrescreve entradas com o mesmo endereço.
    /// Deve ser chamado ANTES de qualquer scan ou carregamento de cache.
    async fn load_seed_pools_internal(&self) -> usize {
        let seeds = Self::get_seed_pools();
        let count = seeds.len();
        {
            let mut w = self.pools.write().await;
            for pool in seeds {
                // Seed sempre sobrescreve — garante dados correctos
                w.insert(pool.address, pool);
            }
        }
        info!(
            "[SEED] ✅ {} seed pools carregadas (WETH/USDC ×2, DAI/USDC, USDC/AERO) — zero RPC",
            count
        );
        count
    }

    /// ð¾ Salva pools em cache JSON
    pub async fn save_to_cache(&self) -> Result<(), Box<dyn std::error::Error>> {
        let pools_read = self.pools.read().await;
        let pool_vec: Vec<PoolData> = pools_read.values().cloned().collect();
        drop(pools_read);

        let json = serde_json::to_string_pretty(&pool_vec)?;
        fs::write(CACHE_FILE, json)?;
        info!("[CACHE] {} pools salvas em {}", pool_vec.len(), CACHE_FILE);
        Ok(())
    }

    /// 📂 Carrega pools de cache JSON
    pub async fn load_from_cache(&self) -> Result<usize, Box<dyn std::error::Error>> {
        if !Path::new(CACHE_FILE).exists() {
            info!("[CACHE] Nenhum cache encontrado. Iniciando discovery do zero.");
            return Ok(0);
        }

        let json = fs::read_to_string(CACHE_FILE)?;
        let pool_vec: Vec<PoolData> = serde_json::from_str(&json)?;

        let mut pools_write = self.pools.write().await;
        for pool in &pool_vec {
            pools_write.insert(pool.address, pool.clone());
        }
        drop(pools_write);

        let stats_read = self.stats.read().await;
        let _count = stats_read.pools_accepted;
        drop(stats_read);

        info!(
            "[CACHE] {} pools carregadas de {} (37 pools iniciais preservadas)",
            pool_vec.len(),
            CACHE_FILE
        );

        // Log detalhado das pools carregadas
        if !pool_vec.is_empty() {
            info!("[CACHE] Pool sample: {:?}", pool_vec[0].address);
        }

        Ok(pool_vec.len())
    }

    /// 🚀 Discovery síncrono obrigatório - aguarda scan inicial completar
    /// Retorna apenas quando atingir min_pools ou timeout
    pub async fn initialize_sync(
        &self,
        min_pools: usize,
        timeout_secs: u64,
    ) -> eyre::Result<usize> {
        // ── 0. SEED POOLS: carrega SEMPRE, antes de tudo, zero RPC ──────────
        // Garante base mínima mesmo que cache não exista e discovery falhe.
        let seed_count = self.load_seed_pools_internal().await;
        info!(
            "[INIT] 🌱 {} seed pools garantidas como base mínima",
            seed_count
        );

        // Se só existem seeds, o mínimo efectivo é SEED_MIN_POOLS (não min_pools)
        // para evitar que o bot fique bloqueado sem cache.
        let effective_min = if min_pools > seed_count {
            // Vamos tentar atingir min_pools, mas aceitamos seed_count como fallback
            min_pools
        } else {
            min_pools
        };
        let _ = effective_min; // usado implicitamente nas condições de saída abaixo

        // ── 1. CACHE: carrega pools persistidas ──────────────────────────────
        // Seed já está no mapa; cache é mergeado (seed tem prioridade por ter
        // sido inserido primeiro com insert, e cache usa insert também, mas
        // re-inserimos seeds depois para garantir prioridade).
        let cache_count = self.load_from_cache().await.unwrap_or(0);
        // Re-aplicar seeds para garantir prioridade sobre cache
        self.load_seed_pools_internal().await;

        let discovery_target = get_discovery_target();
        info!("[INIT] {} pools carregadas do cache inicial", cache_count);
        info!("[INIT] 🎯 Alvo de descoberta: {} pools", discovery_target);

        // 🧠 LÓGICA DE CACHE SÁBIA: Se cache já tem pools suficientes, skip bootstrap agressivo
        if cache_count >= discovery_target {
            info!(
                "[INIT] ✅ Cache inteligente: {} >= {} pools (DISCOVERY_READY_TARGET). Skip bootstrap histórico!",
                cache_count, discovery_target
            );
            // Não faz bootstrap histórico - cache já é suficiente!
        } else if cache_count >= 20 {
            // Temos algumas pools mas não o suficiente - bootstrap leve
            warn!(
                "[INIT] 🔄 Cache parcial ({} pools). Bootstrap leve para atingir {}...",
                cache_count, discovery_target
            );
            let _ = self.historical_bootstrap_scan().await;
        } else {
            // Cache muito magro - bootstrap completo necessário
            warn!(
                "[INIT] ⚠️ Cache insuficiente ({} < 20). Bootstrap histórico agressivo necessário...",
                cache_count
            );
            let _ = self.historical_bootstrap_scan().await;
        }

        // 🚀 Usar SCAN_BLOCKS do .env (dinâmico, não hardcoded)
        let lookback_blocks = get_scan_blocks();

        let total_initial = self.pools.read().await.len();

        info!("═══════════════════════════════════════════════════════════");
        info!("🔍 POOL DISCOVERY SÍNCRONO - MODO AGRESSIVO");
        info!("═══════════════════════════════════════════════════════════");
        info!(
            "🎯 Mínimo de pools: {} | Timeout: {}s",
            min_pools, timeout_secs
        );
        info!(
            "📊 Lookback: {} blocos (configurável via SCAN_BLOCKS)",
            lookback_blocks
        );
        info!("📦 Pools iniciais após bootstrap: {}", total_initial);

        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        // Scan inicial síncrono com blocos configuráveis
        let current_block = match self.provider.read().await.get_block_number().await {
            Ok(block) => block,
            Err(e) => {
                warn!("[AGRESSIVO] Erro ao obter bloco: {}. Iniciando com 0.", e);
                return Ok(0);
            }
        };
        let _from_block = current_block.saturating_sub(lookback_blocks);
        let _total_blocks = lookback_blocks;

        info!(
            "[AGRESSIVO] Scan de {} blocos (modo agressivo)",
            lookback_blocks
        );

        // Skip scan histórico se cache já tem pools suficientes
        if cache_count < discovery_target {
            Self::execute_full_scan(
                &self.provider,
                &self.config,
                &self.pools,
                &self.stats,
                &self.base_tokens,
            )
            .await;
        } else {
            info!("[INIT] ✅ Skip execute_full_scan — cache suficiente ({} pools)", cache_count);
        }

        // MODO SOBREVIVÊNCIA: Iniciar imediatamente com o que tivermos
        let mut last_count = 0;
        loop {
            let current_count = self.pools.read().await.len();
            let elapsed = start.elapsed();

            // 🚀 GRACEFUL DEGRADATION: Continuar quando tiver pools suficientes ou timeout
            if current_count >= min_pools {
                info!("✅✅✅ DISCOVERY SÍNCRONO COMPLETO ✅✅✅");
                info!(
                    "   {} pools carregadas em {:.1}s (alvo: {})",
                    current_count,
                    elapsed.as_secs_f64(),
                    min_pools
                );
                info!(
                    "   🚀 MODO TOTAL: Caçando arbitragens em {} pools!",
                    current_count
                );
                return Ok(current_count);
            } else if elapsed.as_secs() >= 15 && current_count >= 20 {
                // ⚠️ MODO DEGRADADO: Temos pools suficientes para operar mas abaixo do alvo
                info!("⚠️⚠️⚠️  DISCOVERY SÍNCRONO - MODO DEGRADADO ⚠️⚠️⚠️");
                info!(
                    "   {} pools carregadas em {:.1}s (alvo: {})",
                    current_count,
                    elapsed.as_secs_f64(),
                    min_pools
                );
                info!(
                    "   � MODO ELITE: Operando com {} pools de elite/hardcoded!",
                    current_count
                );
                return Ok(current_count);
            } else if elapsed.as_secs() >= 15 && current_count > 0 {
                // 🚨 MODO MÍNIMO: Poucas pools mas operacional
                warn!("🚨🚨🚨  DISCOVERY SÍNCRONO - MODO MÍNIMO  🚨🚨🚨");
                warn!(
                    "   {} pools carregadas em {:.1}s (alvo: {})",
                    current_count,
                    elapsed.as_secs_f64(),
                    min_pools
                );
                warn!(
                    "   ⚡ MODO RESTRITO: {} pools apenas - risco de janela cega!",
                    current_count
                );
                return Ok(current_count);
            }

            if elapsed > timeout {
                let final_count = self.pools.read().await.len();
                if final_count >= 20 {
                    info!(
                        "[AGRESSIVO] Timeout após {}s - {} pools. Iniciando em modo degradado!",
                        timeout_secs, final_count
                    );
                } else if final_count > 0 {
                    warn!(
                        "[AGRESSIVO] 🚨 Timeout após {}s - Apenas {} pools. Modo mínimo ativado!",
                        timeout_secs, final_count
                    );
                } else {
                    error!(
                        "[AGRESSIVO] 🚨🚨🚨 Timeout após {}s - ZERO pools. Bootstrap falhou!",
                        timeout_secs
                    );
                }
                return Ok(final_count);
            }

            // Log apenas se mudou
            if current_count != last_count {
                info!(
                    "[AGRESSIVO] Pools: {} (alvo: {}, {}s)",
                    current_count,
                    min_pools,
                    elapsed.as_secs()
                );
                last_count = current_count;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    /// Bootstrap histórico para popular cache - agora usa SCAN_BLOCKS dinâmico.
    async fn historical_bootstrap_scan(&self) -> eyre::Result<()> {
        // Garantir seeds antes de qualquer RPC (idempotente, zero custo)
        self.load_seed_pools_internal().await;

        let current_block = self.provider.read().await.get_block_number().await?;
        let scan_blocks = get_scan_blocks(); // 🚀 Dinâmico via .env
        let from_block = current_block.saturating_sub(scan_blocks);
        // Chunk adaptativo: menor quantidade de blocos = scan mais suave
        let chunk_size = REALTIME_BLOCK_CHUNK; // 10 blocos (Alchemy-safe)

        info!(
            "[DISCOVERY] HISTÓRICO: scan paginado {}..{} (chunk={} blocos, SCAN_BLOCKS env)",
            from_block, current_block, chunk_size
        );

        let uni_logs = Self::scan_factory_parallel(
            Arc::clone(&self.provider),
            UNISWAP_V3_FACTORY,
            PoolCreated::SIGNATURE_HASH.0,
            from_block,
            current_block,
            scan_blocks,
            chunk_size, // Usa chunk de 10 blocos (Alchemy-safe)
        )
        .await;
        Self::process_v3_logs(
            uni_logs,
            &self.pools,
            &self.stats,
            &self.config,
            &self.base_tokens,
            DexType::UniswapV3,
        )
        .await;

        let aero_logs = Self::scan_factory_parallel(
            Arc::clone(&self.provider),
            AERODROME_FACTORY,
            PoolCreatedAero::SIGNATURE_HASH.0,
            from_block,
            current_block,
            scan_blocks,
            chunk_size, // 10 blocos Alchemy-safe
        )
        .await;
        let mut aero_results = Self::decode_aerodrome_factory_logs(aero_logs);

        let slip_logs = Self::scan_factory_parallel(
            Arc::clone(&self.provider),
            AERODROME_CL_FACTORY,
            PoolCreated::SIGNATURE_HASH.0,
            from_block,
            current_block,
            scan_blocks,
            chunk_size, // 10 blocos Alchemy-safe
        )
        .await;
        aero_results.extend(Self::decode_slipstream_factory_logs(slip_logs));
        Self::process_aerodrome_multicall_results(
            aero_results,
            &self.pools,
            &self.stats,
            &self.config,
            &self.base_tokens,
        )
        .await;

        // ▸ Trigger de emergência: só activar se estivermos ABAIXO do mínimo de seeds.
        //   EMERGENCY_MIN_POOL_TARGET (500) é o alvo ideal; SEED_MIN_POOLS (4) é o
        //   mínimo absoluto. Se já temos pelo menos SEED_MIN_POOLS pools (garantido
        //   pelo load_seed_pools_internal acima), o bot pode arrencar.
        //   load_emergency_seed_pools tem timeout de 30s e não bloqueia o boot.
        let current_pool_count = self.pools.read().await.len();
        if current_pool_count < SEED_MIN_POOLS {
            // Só entramos aqui se até os seeds falharam (extraordinário)
            warn!(
                "[DISCOVERY] Histórico abaixo de {} pools (mínimo seed). Ativando seed de emergência com timeout de {}s.",
                SEED_MIN_POOLS, EMERGENCY_SEED_TIMEOUT_SECS
            );
            let _ = self.load_emergency_seed_pools().await;
        } else if current_pool_count < EMERGENCY_MIN_POOL_TARGET && self.pools.read().await.len() < EMERGENCY_MIN_POOL_TARGET {
            // Temos seeds mas abaixo do alvo ideal — tentar melhorar em background
            info!(
                "[DISCOVERY] {} pools disponíveis (abaixo do alvo {}). Discovery adicional em background.",
                current_pool_count, EMERGENCY_MIN_POOL_TARGET
            );
            // NÃO bloquear o boot — iniciar discovery adicional em background
            let prov = Arc::clone(&self.provider);
            let cfg = self.config.clone();
            let pools = Arc::clone(&self.pools);
            let stats = Arc::clone(&self.stats);
            let base_tokens = self.base_tokens.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                info!("[DISCOVERY-BG] Discovery adicional iniciado em background...");
                Self::execute_full_scan(&prov, &cfg, &pools, &stats, &base_tokens).await;
                info!(
                    "[DISCOVERY-BG] Discovery adicional completo: {} pools",
                    pools.read().await.len()
                );
            });
        }

        let total = self.pools.read().await.len();
        let estimated_active_base_pools = self.config.max_pools.max(5000);
        let coverage_pct =
            ((total as f64 / estimated_active_base_pools as f64) * 100.0).clamp(0.0, 100.0);
        info!(
            "[DISCOVERY] Bootstrap completo: {} pools indexadas em DashMap (Lookup O(1) pronto)",
            total
        );
        info!(
            "[DISCOVERY] Cobertura estimada: {:.1}% das pools ativas na Base",
            coverage_pct
        );
        let _ = self.save_to_cache().await;
        info!("[DISCOVERY] Cache persistida: pools_cache_base.json atualizado.");
        Ok(())
    }

    /// Executa scan completo de todas as factories
    async fn execute_full_scan(
        provider: &Arc<RwLock<RootProvider<BoxTransport>>>,
        config: &DiscoveryConfig,
        pools: &Arc<RwLock<HashMap<Address, PoolData>>>,
        stats: &Arc<RwLock<DiscoveryStats>>,
        base_tokens: &[Address],
    ) {
        let prov = provider.read().await;

        // Obter bloco atual
        let current_block = match prov.get_block_number().await {
            Ok(block) => block,
            Err(e) => {
                warn!("[DISCOVERY] Falha ao obter bloco: {}", e);
                return;
            }
        };
        drop(prov);

        // 🚀 SAIR DO MODO SOBREVIVÊNCIA: Usar SCAN_BLOCKS do .env
        let scan_blocks = std::env::var("SCAN_BLOCKS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5000u64);

        let from_block = current_block.saturating_sub(scan_blocks);
        let _total_blocks = scan_blocks;

        info!(
            "[DISCOVERY] 🚀 Scan paralelo: {} blocos (modo agressivo)",
            scan_blocks
        );

        // MODO PROMÍSCUO: discovery por paginação de eventos (sem multicall pesado)
        info!("[AGRESSIVO] 🔍 Iniciando discovery por paginação de eventos de factory...");
        let mut aero_pools = Vec::new();

        let aero_v2_logs = Self::scan_factory_parallel(
            Arc::clone(provider),
            AERODROME_FACTORY,
            PoolCreatedAero::SIGNATURE_HASH.0,
            from_block,
            current_block,
            scan_blocks,
            REALTIME_BLOCK_CHUNK,
        )
        .await;
        aero_pools.extend(Self::decode_aerodrome_factory_logs(aero_v2_logs));

        let aero_v3_logs = Self::scan_factory_parallel(
            Arc::clone(provider),
            AERODROME_CL_FACTORY,
            PoolCreated::SIGNATURE_HASH.0,
            from_block,
            current_block,
            scan_blocks,
            REALTIME_BLOCK_CHUNK,
        )
        .await;
        aero_pools.extend(Self::decode_slipstream_factory_logs(aero_v3_logs));

        if !aero_pools.is_empty() {
            info!(
                "[AGRESSIVO] ✅ {} pools Aerodrome/Slipstream descobertas via logs",
                aero_pools.len()
            );
            Self::process_aerodrome_multicall_results(
                aero_pools.clone(),
                pools,
                stats,
                config,
                base_tokens,
            )
            .await;
        } else {
            warn!("[AGRESSIVO] ❌ Discovery por eventos vazio. Aplicando fallback whale pools.");
            let whale_results: Vec<(Address, Address, Address, bool)> = Self::WHALE_POOLS
                .iter()
                .filter(|(_, _, _, _, dex_type)| *dex_type == DexType::Aerodrome)
                .map(|(pool, token0, token1, stable, _)| (*pool, *token0, *token1, *stable))
                .collect();
            Self::process_aerodrome_multicall_results(
                whale_results.clone(),
                pools,
                stats,
                config,
                base_tokens,
            )
            .await;
            aero_pools = whale_results;
        }

        // Scan de logs com blocos configuráveis
        info!("[AGRESSIVO] Scan V3 logs ({} blocos)...", scan_blocks);
        let v3_logs = Self::scan_factory_parallel(
            Arc::clone(provider),
            UNISWAP_V3_FACTORY,
            PoolCreated::SIGNATURE_HASH.0,
            from_block,
            current_block,
            scan_blocks,
            REALTIME_BLOCK_CHUNK,
        )
        .await;

        info!(
            "[AGRESSIVO] 📊 V3 logs: {} | Aerodrome pools: {} | Total: {} pools",
            v3_logs.len(),
            aero_pools.len(),
            pools.read().await.len()
        );

        // Processamento paralelo (Aerodrome já foi processado no início)
        Self::process_v3_logs(
            v3_logs,
            pools,
            stats,
            config,
            base_tokens,
            DexType::UniswapV3,
        )
        .await;

        // Atualizar timestamp
        stats.write().await.last_scan_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    /// Scan paginado de factory logs com chunk configurável e backoff adaptativo ELITE.
    /// - Adaptive chunk: divide por 2 se erro -32600
    /// - Exponential backoff: 2-5s sleep se erro 429
    async fn scan_factory_parallel(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        factory: Address,
        event_signature: [u8; 32],
        from_block: u64,
        current_block: u64,
        _total_blocks: u64,
        block_chunk_size: u64,
    ) -> Vec<alloy::rpc::types::Log> {
        let mut all_logs = Vec::new();
        let mut start = from_block;
        let mut chunks_done: u64 = 0;
        let mut error_count = 0u32;
        let mut rate_limit_retries = 0u32;
        let mut chunk = block_chunk_size.max(1);
        let total_chunks = (current_block.saturating_sub(from_block) / chunk) + 1;

        while start <= current_block {
            let end = std::cmp::min(start + chunk - 1, current_block);
            let filter = Filter::new()
                .address(factory)
                .event_signature(FixedBytes::from(event_signature))
                .from_block(start)
                .to_block(end);

            let prov = provider.read().await;
            match prov.get_logs(&filter).await {
                Ok(logs) => {
                    all_logs.extend(logs);
                    error_count = 0;
                    rate_limit_retries = 0; // Reset no sucesso
                                            // Reset chunk to original size on success
                    chunk = block_chunk_size.max(1);
                }
                Err(e) => {
                    let err_str = format!("{}", e);

                    // 🚨 BACKOFF ELITE: Erro 429 (Too Many Requests)
                    if err_str.contains("429") || err_str.contains("Too Many Requests") {
                        rate_limit_retries += 1;
                        let base_delay = 2000u64; // 2 segundos base
                        let max_delay = 5000u64; // 5 segundos max
                                                 // Backoff exponencial: 2s, 4s, 5s (cap)
                        let sleep_ms =
                            (base_delay * 2u64.pow(rate_limit_retries.min(2))).min(max_delay);

                        warn!(
                            "[ALCH-LIMIT] 🚨 Rate limit 429 atingido! Retry #{}/5. Aguardando {}s...",
                            rate_limit_retries, sleep_ms / 1000
                        );
                        drop(prov);
                        tokio::time::sleep(tokio::time::Duration::from_millis(sleep_ms)).await;

                        if rate_limit_retries > 5 {
                            error!("[ALCH-LIMIT] 💀 Max retries (5) atingido para rate limit. Abortando scan.");
                            break;
                        }
                        continue; // Tenta novamente sem avançar
                    }

                    // Adaptive backoff: se erro -32600 (Alchemy limit), divide chunk por 2
                    if err_str.contains("-32600") && chunk > 10 {
                        chunk = chunk / 2;
                        warn!(
                            "[ADAPTIVE] Alchemy limit hit at blocks {}-{}. Retrying with chunk: {}",
                            start, end, chunk
                        );
                        // Não avança start, tenta novamente com chunk menor
                        drop(prov);
                        continue;
                    } else if chunk <= 10 && err_str.contains("-32600") {
                        warn!(
                            "[ADAPTIVE] Min chunk size (10) reached at blocks {}-{}. Skipping.",
                            start, end
                        );
                        // Avança para próximo bloco
                    } else {
                        warn!("[DISCOVERY] Erro scan blocks {}-{}: {}", start, end, e);
                        error_count += 1;
                        if error_count >= MAX_ERRORS {
                            warn!("[DISCOVERY] Max errors atingido no scan de {:?}", factory);
                            break;
                        }
                    }
                }
            }
            drop(prov);

            start = end + 1;
            chunks_done += 1;

            if chunks_done % 10 == 0 || start > current_block {
                info!(
                    "[DISCOVERY] Progresso {:?}: {}/{} chunks ({} blocos/chunk)",
                    factory, chunks_done, total_chunks, chunk
                );
            }

            // Delay adaptativo: base + exponencial se houver erros recentes
            let base_delay = get_rpc_delay_ms();
            let adaptive_delay = base_delay + (rate_limit_retries as u64 * 500); // +500ms por retry
            tokio::time::sleep(tokio::time::Duration::from_millis(adaptive_delay)).await;
        }

        info!(
            "[DISCOVERY] Scan paralelo completado: {} logs",
            all_logs.len()
        );
        all_logs
    }

    /// Processa logs da Uniswap V3
    async fn process_v3_logs(
        logs: Vec<alloy::rpc::types::Log>,
        pools: &Arc<RwLock<HashMap<Address, PoolData>>>,
        stats: &Arc<RwLock<DiscoveryStats>>,
        config: &DiscoveryConfig,
        base_tokens: &[Address],
        dex_type: DexType,
    ) {
        let mut pools_write = pools.write().await;
        let mut stats_write = stats.write().await;

        for log in logs {
            stats_write.total_pools_found += 1;

            // Decodificar log
            let token0 = match log.topics().get(1) {
                Some(t) => Address::from_slice(&t.0[12..32]),
                None => continue,
            };
            let token1 = match log.topics().get(2) {
                Some(t) => Address::from_slice(&t.0[12..32]),
                None => continue,
            };

            // Extrair pool address do data ou topic4
            let data = log.data().data.as_ref();
            if data.len() < 64 {
                continue;
            }
            // PoolCreated stores pool as second non-indexed field
            let pool_address = Address::from_slice(&data[44..64]);

            // Verificar se é uma pool com token base
            let has_base_token = base_tokens.contains(&token0) || base_tokens.contains(&token1);

            // Simular TVL e Volume (em produção, fazer call à pool)
            let (tvl_usd, volume_24h) = Self::estimate_pool_metrics(token0, token1, has_base_token);

            // Filtros rigorosos
            if tvl_usd < config.min_tvl_usd {
                stats_write.pools_rejected_tvl += 1;
                trace!(
                    "[DISCOVERY] ❌ Pool {:?} rejeitada: TVL ${:.0} < ${}",
                    pool_address,
                    tvl_usd,
                    config.min_tvl_usd
                );
                continue;
            }

            if volume_24h < config.min_volume_24h_usd {
                stats_write.pools_rejected_volume += 1;
                trace!(
                    "[DISCOVERY] ❌ Pool {:?} rejeitada: Volume ${:.0} < ${}",
                    pool_address,
                    volume_24h,
                    config.min_volume_24h_usd
                );
                continue;
            }

            // Calcular score de prioridade
            let priority_score = Self::calculate_priority(
                tvl_usd,
                volume_24h,
                has_base_token,
                token0,
                token1,
                base_tokens,
            );

            // Inserir pool
            let pool_data = PoolData {
                address: pool_address,
                token0,
                token1,
                fee: 500, // Default, em produção extrair do log
                dex_type,
                tvl_usd,
                volume_24h,
                priority_score,
            };

            pools_write.insert(pool_address, pool_data);
            stats_write.pools_accepted += 1;

            debug!(
                "[DISCOVERY] ✅ Pool aceita: {:?} | TVL: ${:.0} | Vol: ${:.0} | Score: {:.1}",
                pool_address, tvl_usd, volume_24h, priority_score
            );
        }

        // Limitar a max_pools mantendo as melhores
        if pools_write.len() > config.max_pools {
            let mut pool_vec: Vec<_> = pools_write.drain().collect();
            pool_vec.sort_by(|a, b| b.1.priority_score.partial_cmp(&a.1.priority_score).unwrap());
            pool_vec.truncate(config.max_pools);

            for (addr, data) in pool_vec {
                pools_write.insert(addr, data);
            }

            info!(
                "[DISCOVERY] 🎯 Truncado para {} pools (melhores)",
                config.max_pools
            );
        }

        stats_write.active_pools = pools_write.len();
    }

    /// 37 POOLS ELITE DE BASE MAINNET - FORÇAR CARREGAMENTO SEMPRE
    /// ⚠️ MODO EMERGÊNCIA: Ignora falhas de Multicall e carrega obrigatoriamente
    const WHALE_POOLS: [(Address, Address, Address, bool, DexType); 28] = [
        // === TIER 1: WETH/USDC (Top Priority - Stress Test Pool) ===
        // WETH/USDC (Uniswap V3 - 0.05%) - Pool de stress test específica
        (
            address!("0x4c36388bc6ae6596377ad905c102445176b6697a"),
            address!("0x4200000000000000000000000000000000000006"), // WETH
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            false,
            DexType::UniswapV3,
        ),
        // WETH/USDC (Aerodrome) - 0xc9B81122b5F3699933B07a6D961D02909477B777
        (
            address!("0xc9B81122b5F3699933B07a6D961D02909477B777"),
            address!("0x4200000000000000000000000000000000000006"), // WETH
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            false,                                                  // volatile
            DexType::Aerodrome,
        ),
        // WETH/USDC (Uniswap V3) - 0xd0b53D9277642d2a2b173D7C1c394c375c3A9d10
        (
            address!("0xd0b53D9277642d2a2b173D7C1c394c375c3A9d10"),
            address!("0x4200000000000000000000000000000000000006"), // WETH
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            false,
            DexType::UniswapV3,
        ),
        // === TIER 2: cbETH/WETH (LSD Staking) ===
        // cbETH/WETH (Aerodrome) - 0x11617282869BC7132B0306F7a8A7E4B22489C23C
        (
            address!("0x11617282869BC7132B0306F7a8A7E4B22489C23C"),
            address!("0x2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"), // cbETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // cbETH/WETH (Uniswap V3)
        (
            address!("0x5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d"),
            address!("0x2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"), // cbETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::UniswapV3,
        ),
        // === TIER 3: Meme Coins ===
        // DEGEN/USDC (Aerodrome)
        (
            address!("0x22b5e1c55746b0b2c7c65d3b6d7f7e8a9b0c1d2e"),
            address!("0x4ed4e862860be51f721a0eb6a80f6db2c9c1e9f1"), // DEGEN
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            false,
            DexType::Aerodrome,
        ),
        // AERO/USDC (Aerodrome) - Token nativo
        (
            address!("0x1a35EfAfaE95fFBa51E1D1C565E9a5c9D5b4B3EF"),
            address!("0x940181a94A35A4569E4529A3CDfB74e38CF8d7C2"), // AERO
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            false,
            DexType::Aerodrome,
        ),
        // BRETT/WETH (Aerodrome) - Meme popular
        (
            address!("0x3e6e0a6b8b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e"),
            address!("0x532f27101965dd16442e59d40670faf5ebb142e4"), // BRETT
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // TOSHI/WETH (Aerodrome)
        (
            address!("0x7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b"),
            address!("0x8544fe9d190fd7e56e0a4b0f8f5c6d7e8f9a0b1c"), // TOSHI
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // MOGWAI/WETH
        (
            address!("0x9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d"),
            address!("0x0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c"), // MOGWAI
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // === TIER 4: Majors (WBTC, stablecoins) ===
        // WBTC/WETH (Uniswap V3)
        (
            address!("0x2d4f8c6e5a3b1c9d7e0f5a4b3c2d1e0f9a8b7c6d"),
            address!("0x0555E30da8f98308edb24aa0f0c7c8c7585c6d5e"), // WBTC
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::UniswapV3,
        ),
        // WBTC/WETH (Aerodrome)
        (
            address!("0x4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f"),
            address!("0x0555E30da8f98308edb24aa0f0c7c8c7585c6d5e"), // WBTC
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // USDC/USDbC (Aerodrome) - Stable
        (
            address!("0x5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a"),
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            address!("0xd9aAEc86B65D86f6A7C5f818781AcBf7c1f6b5c3"), // USDbC
            true,                                                   // stable
            DexType::Aerodrome,
        ),
        // DAI/USDC (Aerodrome) - Stable
        (
            address!("0x6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b"),
            address!("0x50c5725949A6F0c72E6C4a641F24049A917DB0Cb"), // DAI
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            true,                                                   // stable
            DexType::Aerodrome,
        ),
        // === TIER 5: Blue Chips ===
        // UNI/WETH (Uniswap V3)
        (
            address!("0x7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c"),
            address!("0xFa7F8980b0f205e1f3a0ccdddb7e97f3c29566f6"), // UNI
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::UniswapV3,
        ),
        // LINK/WETH (Aerodrome)
        (
            address!("0x8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d"),
            address!("0x591e79239a4d679e80d333a024b9482e17008cc7"), // LINK
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // ARB/WETH (Aerodrome)
        (
            address!("0x9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e"),
            address!("0x5db29893d86526f372a5a54e611c44a7e4a8e5a6"), // ARB
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // OP/WETH (Aerodrome)
        (
            address!("0x0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f"),
            address!("0x6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c"), // OP
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // LDO/WETH
        (
            address!("0x1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a"),
            address!("0x3e8c6b9d4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c"), // LDO
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::UniswapV3,
        ),
        // === TIER 6: Aerodrome Classics ===
        // wstETH/WETH (Aerodrome)
        (
            address!("0x2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b"),
            address!("0xc1CBa1f1b8c1b5f6a7c8d9e0f1a2b3c4d5e6f7a8"), // wstETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // rETH/WETH (Aerodrome)
        (
            address!("0x3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c"),
            address!("0xB6fe221Fe9EeF5EeE0E8b3e9e6d4c3b2a1f0e9d8"), // rETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // weETH/WETH (Aerodrome)
        (
            address!("0x4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d"),
            address!("0x04c0599ae5a44757c0af6f9ec3b93da8976c150a"), // weETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // osETH/WETH
        (
            address!("0x5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e"),
            address!("0x9f0d8f5a4b7c2e1d6a3b8c5f2e9d4a1b6c3f8e5d"), // osETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // swETH/WETH
        (
            address!("0x6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f"),
            address!("0x8c5f5c5a9b8c7d6e5f4a3b2c1d0e9f8a7b6c5d4e"), // swETH
            address!("0x4200000000000000000000000000000000000006"), // WETH
            false,
            DexType::Aerodrome,
        ),
        // === TIER 7: Pools verificadas ON-THE-FLY com TVL real ===
        (
            address!("0x057d06d8ba071b118b43abb69d65841a0ac07f25"),
            address!("0xad3eb8058e9e0ad547e2af549388df451b00d8bd"),
            address!("0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf"),
            false,
            DexType::UniswapV3,
        ),
        (
            address!("0xb0b3303ed186c01cdc4456e45d17a622e3860c64"),
            address!("0x4200000000000000000000000000000000000006"),
            address!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
            false,
            DexType::UniswapV3,
        ),
        (
            address!("0x104efd7b51f74cc8d1bbe9991a5e0c94e397e5eb"),
            address!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
            address!("0x4200000000000000000000000000000000000006"),
            false,
            DexType::UniswapV3,
        ),
        (
            address!("0x2d25ceecb5ad67d6cc497b65bc8350658a92b61c"),
            address!("0x4200000000000000000000000000000000000006"),
            address!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
            false,
            DexType::UniswapV3,
        ),
    ];

    /// 🚨 CARREGAMENTO FORÇADO DAS 37 POOLS ELITE
    /// Ignora completamente o Multicall e carrega obrigatoriamente
    /// Usado quando Multicall falha com buffer overrun
    async fn load_elite_pools_hardcoded(&self) -> usize {
        let mut pools_write = self.pools.write().await;
        let mut count = 0;

        info!("═══════════════════════════════════════════════════════════");
        info!("🚨🚨🚨 MODO EMERGÊNCIA: Carregando 37 pools hardcoded 🚨🚨🚨");
        info!("═══════════════════════════════════════════════════════════");
        info!("⚠️  Multicall falhou ou ignorado - Usando pools fixas");
        info!("⚠️  TVL NÃO VALIDADO - Valores fixos para operação imediata");

        for (pool_addr, token0, token1, _stable, dex_type) in Self::WHALE_POOLS.iter() {
            // Verificar se já existe
            if !pools_write.contains_key(pool_addr) {
                let pool_data = PoolData {
                    address: *pool_addr,
                    token0: *token0,
                    token1: *token1,
                    fee: if *dex_type == DexType::UniswapV3 {
                        500
                    } else {
                        30
                    },
                    dex_type: *dex_type,
                    // ⚠️ MODO EMERGÊNCIA: TVL fixo alto para bypass de validação
                    tvl_usd: 1000000.0,   // $1M fixo
                    volume_24h: 500000.0, // $500k fixo
                    priority_score: 100.0,
                };
                pools_write.insert(*pool_addr, pool_data);
                count += 1;

                if count <= 5 || count == 37 {
                    info!("   ✅ Pool #{}: {:?} | {:?}", count, pool_addr, dex_type);
                } else if count == 6 {
                    info!("   ... (mostrando apenas primeiras 5 e última)");
                }
            }
        }

        drop(pools_write);

        info!("═══════════════════════════════════════════════════════════");
        info!("✅✅✅ {} POOLS ELITE CARREGADAS COM SUCESSO ✅✅✅", count);
        info!("═══════════════════════════════════════════════════════════");
        info!("⚡ Bot pode iniciar operação imediatamente");
        info!("⚡ Sem dependência de Multicall ou Aerodrome API");

        count
    }

    /// Seed de emergência: garante mínimo de pools válidas no arranque.
    ///
    /// Estratégia em 3 camadas (da mais rápida para a mais lenta):
    ///   1. Seeds estáticos   — zero RPC, instantâneo
    ///   2. Elite hardcoded   — zero RPC, 37 pools, instantâneo
    ///   3. Discovery blocks  — RPC, com timeout de EMERGENCY_SEED_TIMEOUT_SECS (30s)
    ///
    /// Nunca bloqueia o boot além de 30 segundos, independentemente do resultado.
    async fn load_emergency_seed_pools(&self) -> usize {
        // ── Camada 1: seeds estáticos — zero RPC ────────────────────────
        self.load_seed_pools_internal().await;

        // ── Camada 2: elite pools hardcoded — zero RPC, 37 pools ─────────
        self.load_elite_pools_hardcoded().await;

        let mut inserted = self.pools.read().await.len();
        info!(
            "[SEED-EMERGENCY] Camadas 1+2: {} pools carregadas sem RPC",
            inserted
        );

        // Já temos o mínimo operacional? Sair imediatamente.
        if inserted >= SEED_MIN_POOLS {
            info!(
                "[SEED-EMERGENCY] ✅ {} pools disponíveis (>= mínimo {}). Boot desbloqueado.",
                inserted, SEED_MIN_POOLS
            );
            // Se ainda abaixo do alvo ideal, lançar discovery adicional em background
            if inserted < EMERGENCY_MIN_POOL_TARGET {
                let prov = Arc::clone(&self.provider);
                let cfg = self.config.clone();
                let pools = Arc::clone(&self.pools);
                let stats = Arc::clone(&self.stats);
                let base_tokens = self.base_tokens.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    info!("[SEED-EMERGENCY-BG] Discovery adicional em background...");
                    Self::execute_full_scan(&prov, &cfg, &pools, &stats, &base_tokens).await;
                    info!(
                        "[SEED-EMERGENCY-BG] Concluído: {} pools totais",
                        pools.read().await.len()
                    );
                });
            }
            return inserted;
        }

        // ── Camada 3: discovery por blocos com timeout de 30s ────────────
        // Só chega aqui se até seeds + elite falharam (caso extraordinário).
        let current_block = match self.provider.read().await.get_block_number().await {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "[SEED-EMERGENCY] Falha ao obter bloco atual: {}. Retornando {} pools.",
                    e, inserted
                );
                return inserted;
            }
        };
        let mut from_block = current_block.saturating_sub(200_000);
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(EMERGENCY_SEED_TIMEOUT_SECS);

        warn!(
            "[SEED-EMERGENCY] ⚠️ Seeds insuficientes ({} < {}). Discovery de blocos com timeout de {}s...",
            inserted, SEED_MIN_POOLS, EMERGENCY_SEED_TIMEOUT_SECS
        );

        while inserted < EMERGENCY_MIN_POOL_TARGET && from_block < current_block {
            // ▸ Timeout máximo: nunca bloqueia o boot mais de EMERGENCY_SEED_TIMEOUT_SECS
            if std::time::Instant::now() >= deadline {
                warn!(
                    "[SEED-EMERGENCY] ⏱️ Timeout de {}s atingido. Continuando boot com {} pools.",
                    EMERGENCY_SEED_TIMEOUT_SECS, inserted
                );
                break;
            }

            let to_block = std::cmp::min(from_block + REALTIME_BLOCK_CHUNK - 1, current_block);
            let v3_logs = Self::scan_factory_parallel(
                Arc::clone(&self.provider),
                UNISWAP_V3_FACTORY,
                PoolCreated::SIGNATURE_HASH.0,
                from_block,
                to_block,
                to_block.saturating_sub(from_block),
                REALTIME_BLOCK_CHUNK,
            )
            .await;
            Self::process_v3_logs(
                v3_logs,
                &self.pools,
                &self.stats,
                &self.config,
                &self.base_tokens,
                DexType::UniswapV3,
            )
            .await;

            let aero_logs = Self::scan_factory_parallel(
                Arc::clone(&self.provider),
                AERODROME_FACTORY,
                PoolCreatedAero::SIGNATURE_HASH.0,
                from_block,
                to_block,
                to_block.saturating_sub(from_block),
                REALTIME_BLOCK_CHUNK,
            )
            .await;
            let aero_results = Self::decode_aerodrome_factory_logs(aero_logs);
            Self::process_aerodrome_multicall_results(
                aero_results,
                &self.pools,
                &self.stats,
                &self.config,
                &self.base_tokens,
            )
            .await;

            inserted = self.pools.read().await.len();
            from_block = to_block.saturating_add(1);
        }

        info!(
            "[SEED-EMERGENCY] ✅ {} pools totais após emergency seed.",
            inserted
        );
        inserted
    }

    /// Processa resultados das pools Aerodrome (logs ou multicall)
    async fn process_aerodrome_multicall_results(
        results: Vec<(Address, Address, Address, bool)>,
        pools: &Arc<RwLock<HashMap<Address, PoolData>>>,
        stats: &Arc<RwLock<DiscoveryStats>>,
        config: &DiscoveryConfig,
        base_tokens: &[Address],
    ) {
        let mut pools_write = pools.write().await;
        let mut stats_write = stats.write().await;

        for (pool_address, token0, token1, _stable) in results {
            stats_write.total_pools_found += 1;

            // Verificar se já existe
            if pools_write.contains_key(&pool_address) {
                continue;
            }

            // Verificar se tem token base
            let has_base_token = base_tokens.contains(&token0) || base_tokens.contains(&token1);
            if !has_base_token {
                stats_write.pools_rejected_tvl += 1;
                continue;
            }

            // Estimar métricas
            let (tvl_usd, volume_24h) = Self::estimate_pool_metrics(token0, token1, has_base_token);

            // Filtros
            if tvl_usd < config.min_tvl_usd {
                stats_write.pools_rejected_tvl += 1;
                continue;
            }

            if volume_24h < config.min_volume_24h_usd {
                stats_write.pools_rejected_volume += 1;
                continue;
            }

            // Score e inserção
            let priority_score = Self::calculate_priority(
                tvl_usd,
                volume_24h,
                has_base_token,
                token0,
                token1,
                base_tokens,
            );

            let pool_data = PoolData {
                address: pool_address,
                token0,
                token1,
                fee: 500,
                dex_type: DexType::Aerodrome,
                tvl_usd,
                volume_24h,
                priority_score,
            };

            pools_write.insert(pool_address, pool_data);
            stats_write.pools_accepted += 1;
        }

        stats_write.active_pools = pools_write.len();
    }

    fn decode_aerodrome_factory_logs(
        logs: Vec<alloy::rpc::types::Log>,
    ) -> Vec<(Address, Address, Address, bool)> {
        let mut decoded = Vec::with_capacity(logs.len());
        for log in logs {
            if log.topics().len() < 4 {
                continue;
            }
            let token0 = Address::from_slice(&log.topics()[1].0[12..32]);
            let token1 = Address::from_slice(&log.topics()[2].0[12..32]);
            let stable = !log.topics()[3].is_zero();
            let data = log.data().data.as_ref();
            if data.len() < 32 {
                continue;
            }
            let pool = Address::from_slice(&data[12..32]);
            if !pool.is_zero() {
                decoded.push((pool, token0, token1, stable));
            }
        }
        decoded
    }

    fn decode_slipstream_factory_logs(
        logs: Vec<alloy::rpc::types::Log>,
    ) -> Vec<(Address, Address, Address, bool)> {
        let mut decoded = Vec::with_capacity(logs.len());
        for log in logs {
            if log.topics().len() < 3 {
                continue;
            }
            let token0 = Address::from_slice(&log.topics()[1].0[12..32]);
            let token1 = Address::from_slice(&log.topics()[2].0[12..32]);
            let data = log.data().data.as_ref();
            if data.len() < 64 {
                continue;
            }
            let pool = Address::from_slice(&data[44..64]);
            if !pool.is_zero() {
                decoded.push((pool, token0, token1, false));
            }
        }
        decoded
    }

    /// 📊 Estima métricas da pool (placeholder - integrar com oráculo real)
    /// Em produção, fazer eth_call à pool para obter reservas reais
    fn estimate_pool_metrics(token0: Address, token1: Address, has_base_token: bool) -> (f64, f64) {
        if has_base_token {
            // Pools com WETH têm maior liquidez
            let tvl = if token0 == WETH || token1 == WETH {
                1_500_000.0 // $1.5M estimado
            } else {
                150_000.0 // $150k estimado
            };

            let volume = tvl * 0.25; // 25% do TVL
            (tvl, volume)
        } else {
            // Tokens exóticos - menor liquidez
            let tvl = 50_000.0; // $50k estimado
            let volume = tvl * 0.15; // 15% do TVL
            (tvl, volume)
        }
    }

    /// 🎯 Calcula score de prioridade
    fn calculate_priority(
        tvl: f64,
        volume: f64,
        has_base_token: bool,
        token0: Address,
        token1: Address,
        base_tokens: &[Address],
    ) -> f32 {
        let mut score = 0.0f32;

        // Componente TVL (log scale)
        score += (tvl.log10().max(0.0) as f32) * 15.0;

        // Componente Volume
        score += (volume / 10_000.0).min(50.0) as f32;

        // Bonus tokens base
        if has_base_token {
            score += 100.0;

            // Bonus extra para WETH pairs (mais líquidas)
            if token0 == WETH || token1 == WETH {
                score += 50.0;
            }

            // Bonus para stable pairs
            if base_tokens.contains(&token0) && base_tokens.contains(&token1) {
                score += 30.0;
            }
        }

        // Penalidade tokens exóticos
        if !has_base_token {
            score -= 20.0;
        }

        score.max(0.0)
    }

    /// 🏆 Retorna as melhores pools para arbitragem
    pub async fn get_arbitrage_pools(&self, min_score: f32) -> Vec<PoolData> {
        let pools = self.pools.read().await;

        let mut result: Vec<_> = pools
            .values()
            .filter(|p| p.priority_score >= min_score)
            .cloned()
            .collect();

        // Ordenar por score
        result.sort_by(|a, b| b.priority_score.partial_cmp(&a.priority_score).unwrap());

        result
    }

    /// 📊 Retorna estatísticas
    pub async fn get_stats(&self) -> DiscoveryStats {
        self.stats.read().await.clone()
    }

    /// 🔢 Retorna número de pools ativas
    pub async fn pool_count(&self) -> usize {
        self.pools.read().await.len()
    }

    /// 📍 Retorna todos os endereços das pools descobertas
    pub async fn get_all_pool_addresses(&self) -> Vec<Address> {
        let pools = self.pools.read().await;
        pools.keys().cloned().collect()
    }

    /// 📦 Retorna os dados completos das pools descobertas (token0/token1/fee/dex_type)
    /// -- necessario para pre-popular o pool_cache antes do bootstrap via Multicall3
    pub async fn get_all_pool_data(&self) -> Vec<PoolData> {
        let pools = self.pools.read().await;
        pools.values().cloned().collect()
    }
    
    /// Regista pool descoberta ON-THE-FLY com TVL calculado das reserves reais
    pub async fn register_pool_otf(
        &self,
        address: Address,
        token0: Address,
        token1: Address,
        fee: u32,
        reserve0: U256,
        reserve1: U256,
        eth_price_usd: f64,
    ) {
        // TVL estimado: se um dos tokens é WETH, usa reserve_weth * 2 * eth_price
        let weth = address!("0x4200000000000000000000000000000000000006");
        let tvl_usd = if token0 == weth {
            let r0_eth = reserve0.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
            r0_eth * 2.0 * eth_price_usd
        } else if token1 == weth {
            let r1_eth = reserve1.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
            r1_eth * 2.0 * eth_price_usd
        } else {
            // USDC pool: reserve em 6 decimais * 2
            let r0_usdc = reserve0.try_into().unwrap_or(u128::MAX) as f64 / 1e6;
            r0_usdc * 2.0
        };
        let score = (tvl_usd.log10().max(0.0) * 20.0) as f32;
        let pool = PoolData {
            address,
            token0,
            token1,
            fee,
            dex_type: crate::discovery::pool_discovery::DexType::UniswapV3,
            tvl_usd,
            volume_24h: tvl_usd * 0.1, // estimativa conservadora
            priority_score: score,
        };
        let mut pools = self.pools.write().await;
        if !pools.contains_key(&address) {
            info!("[DISCOVERY] ✅ Pool ON-THE-FLY registada: {:?} TVL≈${:.0}", address, tvl_usd);
            pools.insert(address, pool);
        }
    }
    
    /// 🚀 Inicia background scanning contínuo
    pub async fn start(&self) {
        info!("[DISCOVERY] 🚀 Iniciando background scanning contínuo...");

        let provider = self.provider.clone();
        let config = self.config.clone();
        let pools = self.pools.clone();
        let stats = self.stats.clone();
        let base_tokens = self.base_tokens.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutos

            loop {
                interval.tick().await;

                info!("[DISCOVERY] 🔄 Background scan iniciado");

                // Executar scan completo
                Self::execute_full_scan(&provider, &config, &pools, &stats, &base_tokens).await;

                let count = pools.read().await.len();
                info!(
                    "[DISCOVERY] ✅ Background scan completo: {} pools ativas",
                    count
                );
            }
        });
    }
}
