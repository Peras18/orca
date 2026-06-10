#![deny(warnings)]

use std::sync::Arc;
use tracing::{info, error};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_appender::non_blocking::WorkerGuard;

use apex_base_mev::artemis::{
    ArtemisEngine, LogCollector, LogFilter, CollectorConfig, StrategyContext,
    ApexPredatorEngine, ApexConfig,
};
use apex_base_mev::strategist::{HighPerformanceStrategist, StrategistConfig};
use apex_base_mev::contracts::{UniswapV3Factory, AerodromeFactory, DexType};
use apex_base_mev::strategist::ProfitConfig;
use apex_base_mev::provider::Provider;
use apex_base_mev::config::AppConfig;
use apex_base_mev::EngineConfig;
use apex_base_mev::discovery::{PoolDiscoveryEngine, DiscoveryConfig};
use apex_base_mev::god_mode::GodModeEngine;
use apex_base_mev::apex_shadow_protocol::ApexShadowProtocol;
use apex_base_mev::telemetry::{TelemetryCollector, spawn_telemetry_printer};
use apex_base_mev::empire::EmpireFoundationEngine;
use apex_base_mev::singularity::SingularityMEV;

/// Verifica Chain ID e moradas das factories
fn verify_chain_configuration() {
    const EXPECTED_CHAIN_ID: u64 = 8453;
    info!("Chain ID: {} (Base Mainnet)", EXPECTED_CHAIN_ID);
    info!("Uniswap V3 Factory: {:?}", UniswapV3Factory::ADDRESS);
    info!("Aerodrome Factory: {:?}", AerodromeFactory::BASE_MAINNET);
}

fn setup_logging() -> WorkerGuard {
    std::fs::create_dir_all("logs").ok();
    
    let file_appender = tracing_appender::rolling::never("logs", "mev_results.log");
    let (non_blocking_file, file_guard) = tracing_appender::non_blocking(file_appender);
    
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true);
    
    let env_filter = EnvFilter::new("apex_base_mev=debug,info");
    
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);
    
    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();
    
    info!("Logger inicializado: logs/mev_results.log");
    
    file_guard
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let app_config = AppConfig::load();
    let _file_guard = setup_logging();

    let debug_mode = std::env::var("DEBUG_MODE").unwrap_or_default() == "true";
    if debug_mode {
        info!("MODO DEBUG: Threshold 0.0000625 ETH (~0.10€)");
        info!("Logging detalhado ativado");
    }

    info!("ApexBaseMEV Bot - Iniciando");
    verify_chain_configuration();

    let config = EngineConfig {
        region: Box::leak(app_config.region.clone().into_boxed_str()),
        max_path_length: app_config.max_path_length,
        min_profit_basis_points: app_config.min_profit_basis_points,
        dry_run: app_config.dry_run,
        enable_backrun: app_config.enable_backrun,
    };
    
    if config.dry_run {
        info!("MODO SIMULACAO: Sem execucao real");
    }

    let provider = Arc::new(Provider::new(&config, &app_config).await?);
    
    let telemetry_collector = Arc::new(TelemetryCollector::new());
    tokio::spawn(spawn_telemetry_printer(telemetry_collector.clone()));
    info!("Telemetria ativada");
    
    // DISCOVERY SINCRONO OBRIGATORIO
    let discovery_config = DiscoveryConfig {
        min_tvl_usd: 10_000.0,
        min_volume_24h_usd: 5_000.0,
        max_pools: 500,
        scan_interval_secs: 300,
        lookback_blocks: 17280,
    };
    
    let discovery_engine = Arc::new(PoolDiscoveryEngine::new(
        provider.inner().clone(),
        discovery_config,
    ));
    
    // AGUARDAR 500+ POOLS ANTES DE CONTINUAR
    info!("DISCOVERY SINCRONO: Aguardando 500+ pools...");
    let pool_count = discovery_engine.initialize_sync(500, 120).await?;
    info!("Discovery completo: {} pools carregadas", pool_count);
    
    // Background scanning apos sincrono
    discovery_engine.start().await;
    
    // GodModeEngine
    let god_mode_engine = GodModeEngine::new(provider.inner().clone());
    god_mode_engine.spawn().await?;

    // ApexShadowProtocol
    let mev_broadcaster = Arc::new(apex_base_mev::executor::MevShareBroadcaster::new(
        provider.inner().clone()
    ));
    let apex_protocol = ApexShadowProtocol::new(
        provider.inner().clone(),
        mev_broadcaster,
    );
    apex_protocol.spawn().await?;

    let collector_config = CollectorConfig {
        wss_url: app_config.base_wss_url.clone(),
        max_concurrent_logs: 10000,
        channel_capacity: 10000,
    };

    let log_filter = LogFilter {
        target_pools: Vec::new(),
        target_tokens: Vec::new(),
        enabled_dexes: vec![DexType::UniswapV3, DexType::Aerodrome, DexType::PancakeSwap],
    };

    let collector = Arc::new(LogCollector::new(
        provider.inner().clone(),
        log_filter,
        collector_config,
    ));

    let strategist_config = StrategistConfig {
        max_path_length: config.max_path_length,
        min_profit_bps: config.min_profit_basis_points,
        max_gas_price_gwei: 500,
        update_batch_size: 1000,
    };
    
    let profit_config = ProfitConfig {
        flash_loan_fee_bps: 30,
        gas_price_gwei: 1,
        safety_margin_bps: 100,
        max_iterations: 10,
    };
    
    info!("Gas configurado: {} gwei", profit_config.gas_price_gwei);
    info!("Max Gas: {} gwei", strategist_config.max_gas_price_gwei);

    let executor_address = app_config.executor_address();
    let strategist = HighPerformanceStrategist::new(strategist_config, executor_address, profit_config);

    let strategy_context = StrategyContext {
        executor_address,
        max_gas_price: 100_000_000_000,
        max_slippage_bps: 100,
        min_profit_eth: 2_000_000_000_000_000,
        eth_price_usd: 3500.0,
        priority_fee_gwei: 2,
        max_priority_fee_gwei: 50,
        max_reaction_time_ms: 100,
        dry_run: true,
    };

    // ApexPredatorEngine
    let apex_engine = Arc::new(ApexPredatorEngine::new(ApexConfig {
        max_cycle_hops: 5,
        min_cycle_hops: 3,
        liquidation_threshold_eth: 1.0,
        max_base_fee_gwei: 100,
        priority_tip_multiplier: 1.5,
        max_parallel_simulations: 20,
        cycle_reaction_time_us: 500,
    }));
    
    info!("ApexPredatorEngine ativado");
    
    // EmpireFoundationEngine
    let empire_engine = Arc::new(EmpireFoundationEngine::new());
    
    let yul_benchmarks = empire_engine.yul_optimizer.benchmark_all();
    for benchmark in &yul_benchmarks {
        info!(
            "[YUL] {}: Gas {} -> {} (Economia: {:.1}%)",
            benchmark.operation,
            benchmark.standard_gas,
            benchmark.optimized_gas,
            benchmark.savings_percent
        );
    }
    
    info!("EmpireFoundationEngine ativado");
    
    // SingularityMEV
    let singularity = Arc::new(SingularityMEV::new().await);
    singularity.sequencer_monitor.start_monitoring().await;
    
    info!("SingularityMEV ativado");
    
    // ArtemisEngine - Motor principal
    let mut engine = ArtemisEngine::new(
        config,
        provider.clone(),
        collector.clone(),
        strategist,
        strategy_context,
    );

    info!("");
    info!("========================================");
    info!("MEV Engine pronto - Aguardando eventos");
    info!("========================================");
    info!("");

    engine.run().await?;

    Ok(())
}
