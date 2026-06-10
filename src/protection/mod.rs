//! 🛡️ Protection Module — Honeypot detection, circuit breaker, balance tracking,
//! wallet rotation, gas fingerprinting, timing jitter, exact output

pub mod honeypot_detector;
pub use honeypot_detector::{HoneypotDetector, HoneypotResult};

pub mod circuit_breaker;
pub use circuit_breaker::{CircuitBreaker, CircuitState, BreakerStats, run_circuit_maintenance};

pub mod balance_tracker;
pub use balance_tracker::{BalanceTracker, StrategyMetrics};

pub mod wallet_rotator;
pub use wallet_rotator::{WalletRotator, RotatorWallet, get_submission_wallet};

pub mod gas_fingerprinter;
pub use gas_fingerprinter::{GasFingerprinter, GasFingerprintConfig};

pub mod timing_jitter;
pub use timing_jitter::{TimingJitter, TimingJitterConfig};

pub mod exact_output;
pub use exact_output::{SwapMode, compute_exact_input_for_output, compute_backwards_multi_hop, is_exact_output_viable};
