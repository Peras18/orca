use alloy::primitives::Address;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenRisk { Safe, TransferTax(u32), SellBlocked, Unknown }

#[derive(Debug)]
pub struct HoneypotFilter {
    cache: RwLock<HashMap<Address, TokenRisk>>,
}

impl HoneypotFilter {
    pub fn new() -> Self {
        Self { cache: RwLock::new(HashMap::new()) }
    }
    pub fn is_safe(&self, _token: Address) -> bool { true }
    pub fn is_path_safe(&self, _tokens: &[Address]) -> bool { true }
    pub fn mark_safe(&self, token: Address) {
        self.cache.write().unwrap().insert(token, TokenRisk::Safe);
    }
    pub fn mark_dangerous(&self, token: Address, risk: TokenRisk) {
        self.cache.write().unwrap().insert(token, risk);
    }
}