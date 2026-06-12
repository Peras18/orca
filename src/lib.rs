#![allow(dead_code)]

pub mod provider;
pub mod pathfinder;
pub mod sim;
pub mod types;
pub mod engine;
pub mod contracts;
pub mod artemis;
pub mod strategist;
pub mod simulator;
pub mod executor;
pub mod deployer;
pub mod config;
pub mod factory_scanner;
pub mod god_mode;
pub mod apex_shadow_protocol;
pub mod telemetry;
pub mod logging;
pub mod empire;
pub mod discovery;
pub mod singularity;
pub mod ghost;
pub mod safety;
pub mod orca;
pub mod liquidation;

// Novos módulos MEV Elite
pub mod cache;
pub mod graph;
pub mod math;
pub mod protection;
pub mod gas;
pub mod strategies;
pub mod metrics;
pub mod risk;
pub mod prediction;

pub use liquidation::{LiquidationMonitor, LiquidationOpportunity};

// Re-exports dos novos módulos
pub use cache::{PoolCache, PoolState, CacheStats, MULTICALL3_ADDRESS};
pub use graph::{ArbGraph, ArbPath, Edge, GraphStats};
pub use math::{get_k_stable, get_y_stable, get_amount_out_stable, get_amount_out_v2};
pub use protection::{
    HoneypotDetector, HoneypotResult,
    CircuitBreaker, CircuitState, BalanceTracker,
    WalletRotator, GasFingerprinter, GasFingerprintConfig,
    TimingJitter, TimingJitterConfig,
    SwapMode, compute_exact_input_for_output,
};
pub use gas::GasOracle;
pub use strategies::{SwapEventFilter, MidCapScanner, JITMonitor};
pub use metrics::MetricsCollector;
pub use risk::BankrollManager;
pub use prediction::PatternMemory;

pub use empire::{
    EmpireFoundationEngine,
    YulOptimizer, FailedStateSpeculator, MultiCallBundler, BytecodeAnalyzer,
};

pub use singularity::{
    SingularityMEV,
    SequencerHeartbeatMonitor, AtomicStateLock, 
    // BridgeShadowPrediction, InvisibleProber, // TODO: Módulos não implementados
    // Shadow-Speculator
    ShadowSpeculator, ShadowMempool, ShadowPendingTx, VirtualPoolState,
    ExoticRouteFinder, ExoticRoute, ExoticEdge,
    PrivacyBundleSender, PrivateBundle,
    ReactivePGA, ShadowOpportunity,
};

pub use ghost::{
    GhostStateEngine, CallbackHijacker, TransientOracle, AtomicMultiAction,
    GhostSwapParams, TargetProtocol, GhostExecution,
};

pub use safety::{
    SafetyEngine, MevShareExecutor, ProfitAdaptiveEngine, KillSwitch, CapitalGuard,
    SystemStatus, RiskLevel, BundleStatus,
};

pub use artemis::{
    LogCollector, LogFilter, CollectorConfig,
    LogCollectorV2, CollectorConfigV2, EventFilter, CollectorMetrics,
    ArtemisEngine, Strategy, StrategyContext,
    ApexPredatorEngine, ApexConfig, ApexOpportunityType,
    // Novos módulos institutionários
    MempoolSniffer, SniffedTransaction, DecodedRoute, SnifferStats,
    KNOWN_MEV_BOTS, RouteStats,
};

// Novos módulos de alta agressividade
pub use executor::{
    multi_call_bundler::{
        BundleBuilder, BundleConfig, BundlePackage, WhaleDetector,
        WHALE_MIN_ETH, MIN_PROFIT_PER_TRADE, PROFIT_AGGRESSIVE, PROFIT_EXTREME,
        GAS_TIP_MIN, GAS_TIP_AGGRESSIVE, GAS_TIP_EXTREME,
    },
    gas_auction::{
        GasAuctionController, GasBid, GasStrategy, PGAStats, BidSimulation,
        PGA_PROFIT_THRESHOLD_EUR, PGA_MAX_GAS_EUR, PGA_AGGRESSIVE_GAS_EUR,
    },
};

pub use strategist::{
    apex_predator::{
        ApexPredator, DailyStats, ExecutionPriority, OpportunityEvaluation,
        CompletedTrade, TradeType,
        MIN_PROFIT_PER_TRADE as APEX_MIN_PROFIT,
        PROFIT_AGGRESSIVE as APEX_PROFIT_AGGR,
        PROFIT_EXTREME as APEX_PROFIT_EXTREME,
        DAILY_TARGET_EUR, DAILY_TARGET_USD,
    },
    whale_predictor::{
        WhalePredictor, WhalePrediction, PoolReserves, PostWhaleArbitrage,
        WHALE_THRESHOLD_ETH, EXECUTION_WINDOW_MS,
    },
    multi_hop_engine::{
        MultiHopEngine, MultiHopArbitragePath, SwapSimulation,
        FlashloanMultiHop, FlashloanProvider,
        MAX_ITERATIONS, PRECISION_WEI,
    },
    newton_jacobian_solver::{
        NewtonJacobianSolver, TriangularSystem, SolverResult, TriangularSystemBuilder,
        FLASHLOAN_FEE_AAVE_BPS, FLASHLOAN_FEE_UNISWAP_BPS, FLASHLOAN_FEE_BALANCER_BPS,
        MIN_PROFIT_LIQUIDO_USD, TARGET_PROFIT_DAILY_EUR,
    },
    continuous_engine::{
        ContinuousProfitEngine, GasSensitivityController, TradeProbability,
        REDUCED_PROFIT_THRESHOLD_SMALL, REDUCED_PROFIT_THRESHOLD_MEDIUM, REDUCED_PROFIT_THRESHOLD_LARGE,
        PendingTxInfo, RecursiveOpportunity, ContinuousStats,
    },
};


pub use orca::{
    OrcaEngine, OrcaConfig, GhostStateExecutor, SequencerSync,
    YulExecutor, PerformanceTracker,
    Opportunity, OpportunityType, ExecutionReceipt, OrcaSystemStatus,
};

pub use provider::Provider;
pub use pathfinder::Pathfinder;
pub use sim::Simulator;
pub use engine::Engine;

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub region: &'static str,
    pub max_path_length: usize,
    pub min_profit_basis_points: u32,
    pub dry_run: bool,  // Modo Shadow Hunter: simula sem executar
    pub enable_backrun: bool,  // Habilitar state overlay para backrunning
}
pub mod logger;
pub mod security;
pub mod notifications;
