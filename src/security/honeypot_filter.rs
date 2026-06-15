use alloy::primitives::Address;
use std::collections::HashMap;
use std::sync::RwLock;
use tracing::warn;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenRisk {
    Safe,
    TransferTax(u32), // bps
    SellBlocked,
    Unknown,
    Honeypot,
}

#[derive(Debug)]
pub struct HoneypotFilter {
    cache: RwLock<HashMap<Address, TokenRisk>>,
}

// Tokens verificados e seguros na Base
const SAFE_TOKENS: &[&str] = &[
    "0x4200000000000000000000000000000000000006", // WETH
    "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", // USDC
    "0x50c5725949a6f0c72e6c4a641f24049a917db0cb", // DAI
    "0x2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEC22", // cbETH
    "0x940181a94A35A4569E4529A3CDfB74e38FD98631", // AERO
    "0xd9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA", // USDbC
    "0xc1CBa3fCea344f92D9239c08C0568f6F2F0ee452", // wstETH
    "0x0000000000000000000000000000000000000000", // zero (skip)
];

impl HoneypotFilter {
    pub fn new() -> Self {
        let mut cache = HashMap::new();
        // Pre-marcar tokens seguros conhecidos
        for addr_str in SAFE_TOKENS {
            if let Ok(addr) = addr_str.parse::<Address>() {
                cache.insert(addr, TokenRisk::Safe);
            }
        }
        Self { cache: RwLock::new(cache) }
    }

    pub fn is_safe(&self, token: Address) -> bool {
        match self.cache.read().unwrap().get(&token) {
            Some(TokenRisk::Safe) => true,
            Some(TokenRisk::Honeypot) | Some(TokenRisk::SellBlocked) => false,
            Some(TokenRisk::TransferTax(bps)) => *bps < 500, // aceitar até 5% de tax
            _ => true, // Unknown: assumir seguro por defeito, aprender com tempo
        }
    }

    pub fn is_path_safe(&self, tokens: &[Address]) -> bool {
        tokens.iter().all(|t| self.is_safe(*t))
    }

    pub fn mark_safe(&self, token: Address) {
        self.cache.write().unwrap().insert(token, TokenRisk::Safe);
    }

    pub fn mark_dangerous(&self, token: Address, risk: TokenRisk) {
        warn!("[HONEYPOT] 🚨 Token marcado como perigoso: {:?} | Risco: {:?}", token, risk);
        self.cache.write().unwrap().insert(token, risk);
    }

    /// Verificar se uma transação falhada indica honeypot
    /// Chamar após cada txn falhada para aprender
    pub fn record_failed_execution(&self, token: Address) {
        let mut cache = self.cache.write().unwrap();
        let entry = cache.entry(token).or_insert(TokenRisk::Unknown);
        if *entry == TokenRisk::Unknown {
            *entry = TokenRisk::SellBlocked;
            warn!("[HONEYPOT] ⚠️ Token suspeito após falha: {:?}", token);
        }
    }

    pub fn stats(&self) -> String {
        let cache = self.cache.read().unwrap();
        let safe = cache.values().filter(|r| **r == TokenRisk::Safe).count();
        let dangerous = cache.values().filter(|r| matches!(r, TokenRisk::Honeypot | TokenRisk::SellBlocked)).count();
        format!("Tokens: {} safe | {} dangerous | {} total", safe, dangerous, cache.len())
    }
}
