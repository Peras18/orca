//! Configuração do ORCA MEV Bot
//!
//! Pools de teste e produção para a Base network

pub mod app_config;
pub mod top_pools_base;

pub use app_config::*;
pub use top_pools_base::{TOP_5_POOLS_BASE, TEST_2_POOLS, TEST_1_POOL};
