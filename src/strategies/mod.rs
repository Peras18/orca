//! 📈 Strategies Module — Backrun, JIT, Long-tail MEV

pub mod backrun;
pub use backrun::{SwapEventFilter, LargeSwapEvent, BatchBuilder, BackrunStats};

pub mod jit_liquidity;
pub use jit_liquidity::{JITMonitor, JITOpportunity};

pub mod long_tail;
pub use long_tail::{MidCapScanner, LaunchMonitor, PriceDivergence};
pub mod liquidation_hunter;
