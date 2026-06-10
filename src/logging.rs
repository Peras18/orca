//! Sistema de Logging de Elite para HFT MEV
//! 
//! Prefixos visuais:
//! - [INIT]   🚀 Inicialização
//! - [FAST]   ⚡ Performance crítica (<1ms)
//! - [WIN]    🎯 Execução lucrativa
//! - [SIM]    🔮 Simulação
//! - [DNA]    🧬 Análise de contrato
//! - [WATCH]  👀 Monitorização
//! - [RISK]   ⚠️  Gestão de risco
//! - [FATAL]  💀 Erro crítico
//! - [HFT]    📊 Métricas de latência

/// Cores ANSI para logs elites
pub mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
    pub const WHITE: &str = "\x1b[37m";
    pub const BRIGHT_GREEN: &str = "\x1b[92m";
    pub const BRIGHT_YELLOW: &str = "\x1b[93m";
    pub const BRIGHT_BLUE: &str = "\x1b[94m";
    pub const BRIGHT_MAGENTA: &str = "\x1b[95m";
    pub const BRIGHT_CYAN: &str = "\x1b[96m";
}

/// Macro para log de inicialização
#[macro_export]
macro_rules! log_init {
    ($($arg:tt)*) => {
        tracing::info!("{}[INIT]{} 🚀 {}", 
            $crate::logging::colors::BRIGHT_CYAN,
            $crate::logging::colors::RESET,
            format!($($arg)*)
        )
    };
}

/// Macro para log de performance rápida
#[macro_export]
macro_rules! log_fast {
    ($latency_us:expr, $($arg:tt)*) => {
        tracing::info!("{}[FAST]{} ⚡ {}µs | {}", 
            $crate::logging::colors::BRIGHT_GREEN,
            $crate::logging::colors::RESET,
            $latency_us,
            format!($($arg)*)
        )
    };
}

/// Macro para log de execução lucrativa
#[macro_export]
macro_rules! log_win {
    ($profit:expr, $($arg:tt)*) => {
        tracing::info!("{}[WIN]{} 🎯 +{:.4} ETH | {}", 
            $crate::logging::colors::BRIGHT_GREEN,
            $crate::logging::colors::RESET,
            $profit,
            format!($($arg)*)
        )
    };
}

/// Macro para log de simulação
#[macro_export]
macro_rules! log_sim {
    ($($arg:tt)*) => {
        tracing::info!("{}[SIM]{} 🔮 {}", 
            $crate::logging::colors::BRIGHT_MAGENTA,
            $crate::logging::colors::RESET,
            format!($($arg)*)
        )
    };
}

/// Macro para log de DNA scan
#[macro_export]
macro_rules! log_dna {
    ($elapsed_us:expr, $($arg:tt)*) => {
        tracing::info!("{}[DNA]{} 🧬 {}µs | {}", 
            $crate::logging::colors::CYAN,
            $crate::logging::colors::RESET,
            $elapsed_us,
            format!($($arg)*)
        )
    };
}

/// Macro para log de monitorização
#[macro_export]
macro_rules! log_watch {
    ($($arg:tt)*) => {
        tracing::info!("{}[WATCH]{} 👀 {}", 
            $crate::logging::colors::BLUE,
            $crate::logging::colors::RESET,
            format!($($arg)*)
        )
    };
}

/// Macro para log de risco
#[macro_export]
macro_rules! log_risk {
    ($($arg:tt)*) => {
        tracing::warn!("{}[RISK]{} ⚠️  {}", 
            $crate::logging::colors::BRIGHT_YELLOW,
            $crate::logging::colors::RESET,
            format!($($arg)*)
        )
    };
}

/// Macro para log fatal
#[macro_export]
macro_rules! log_fatal {
    ($($arg:tt)*) => {
        tracing::error!("{}[FATAL]{} 💀 {}", 
            $crate::logging::colors::RED,
            $crate::logging::colors::RESET,
            format!($($arg)*)
        )
    };
}

/// Macro para log de métricas HFT
#[macro_export]
macro_rules! log_hft {
    ($($arg:tt)*) => {
        tracing::info!("{}[HFT]{} 📊 {}", 
            $crate::logging::colors::BRIGHT_BLUE,
            $crate::logging::colors::RESET,
            format!($($arg)*)
        )
    };
}

/// Banner de inicialização do sistema
pub fn print_banner() {
    use colors::*;
    println!();
    println!("{}╔══════════════════════════════════════════════════════════════════╗{}", BRIGHT_CYAN, RESET);
    println!("{}║           APEX BASE MEV - HFT EXECUTION ENGINE                   ║{}", BRIGHT_CYAN, RESET);
    println!("║                                                                  ║");
    println!("║  Latência: <100us DNA Scan | <1ms Simulação                     ║");
    println!("║  Estratégia: Triangular Arbitrage + Liquidation Hunter          ║");
    println!("║  Segurança: Capital 80 EUR | Kill-Switch 50% | Simulação 100%   ║");
    println!("{}╚══════════════════════════════════════════════════════════════════╝{}", BRIGHT_CYAN, RESET);
    println!();
}

/// Log de startup summary
pub fn print_startup_config(
    chain_id: u64,
    min_profit_eth: f64,
    gas_cap_gwei: u64,
    dry_run: bool,
) {
    use colors::*;
    
    println!("{}[CONFIG]{} ⚙️  Configuração do Sistema:", BRIGHT_BLUE, RESET);
    println!("  {}⛓️  Chain ID: {}{}", CYAN, chain_id, RESET);
    println!("  {}💰 Min Profit: {:.5} ETH ({}€){}", CYAN, min_profit_eth, min_profit_eth * 1600.0, RESET);
    println!("  {}⛽ Gas Cap: {} Mwei ({} Gwei){}", CYAN, gas_cap_gwei, gas_cap_gwei as f64 / 1000.0, RESET);
    println!("  {}🧪 Dry Run: {}{}", CYAN, if dry_run { "SIM" } else { "NÃO" }, RESET);
    
    if dry_run {
        println!("  {}⚠️  MODO SIMULAÇÃO - Nenhuma transação real será enviada{}", BRIGHT_YELLOW, RESET);
    }
    println!();
}

/// Formata latência com cor baseado no valor
pub fn format_latency(latency_us: u128) -> String {
    use colors::*;
    let color = if latency_us < 100 {
        BRIGHT_GREEN
    } else if latency_us < 1000 {
        GREEN
    } else if latency_us < 10000 {
        YELLOW
    } else {
        RED
    };
    
    format!("{}{}µs{}", color, latency_us, RESET)
}

/// Formata profit com cor
pub fn format_profit(profit_eth: f64) -> String {
    use colors::*;
    if profit_eth > 0.0 {
        format!("{}+{:.4} ETH{}", BRIGHT_GREEN, profit_eth, RESET)
    } else if profit_eth < 0.0 {
        format!("{}{:.4} ETH{}", RED, profit_eth, RESET)
    } else {
        format!("{}0.0000 ETH{}", YELLOW, RESET)
    }
}
