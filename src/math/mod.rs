//! 🧮 Math Module — Cálculos financeiros sem f64

pub mod stable;
pub use stable::{get_k_stable, get_y_stable, get_amount_out_stable};

pub mod v2;
pub use v2::{get_amount_out_v2, get_amount_in_v2};

pub mod v3;
pub use v3::get_amount_out as get_amount_out_v3;
pub mod kalman_gas;
pub mod transfer_entropy;
pub mod flash_optimizer;
pub mod spectral_graph;
