//! 💰 Risk Management Module
//!
//! Gestão adaptativa da banca, circuit breakers, e proteção contra perdas.

pub mod bankroll_manager;

pub use bankroll_manager::BankrollManager;
