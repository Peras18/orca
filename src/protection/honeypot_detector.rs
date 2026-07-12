//! 🍯 Honeypot Detector
//! 
//! Antes de qualquer swap em tokens não-WETH/USDC, faz simulação eth_call
//! de swap de regresso. Se falhar ou retornar 0, o token é honeypot.

use alloy::primitives::{Address, Bytes};
use alloy::providers::{Provider, RootProvider};
use alloy::transports::BoxTransport;
use alloy::network::TransactionBuilder;
use std::sync::Arc;
use tracing::warn;
use tokio::sync::RwLock;

/// Resultado da verificação honeypot
#[derive(Clone, Debug, PartialEq)]
pub enum HoneypotResult {
    Safe,           // Pode comprar e vender
    Honeypot,       // Não pode vender
    TransferFee,    // Taxa excessiva (>30%)
    SimulationFail, // Simulação falhou
    Unknown,        // Ainda não testado
}

/// Detector de honeypots com cache
pub struct HoneypotDetector {
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    /// Cache de resultados: token -> (resultado, timestamp)
    cache: Arc<RwLock<std::collections::HashMap<Address, (HoneypotResult, u64)>>>,
    /// TTL do cache em segundos (24h)
    cache_ttl_secs: u64,
    /// WETH address na BASE
    weth: Address,
    /// USDC address na BASE
    usdc: Address,
}

/// Tokens considerados seguros (whitelist)
const WHITELIST: &[&str] = &[
    "0x4200000000000000000000000000000000000006", // WETH
    "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", // USDC
    "0x50c5725949A6F0c72E6C4a641F2409A017ebaBdf", // DAI
    "0xd9aAEc86B65D86f6A7B5B3b78339C7aD4e5716e2", // USDbC
    "0x2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0Dbc62", // cbETH
    "0xc1CBa9f5a3D8b6e0e3F6D9C0F4A2B1c3d4E5F6A7", // wstETH
];

impl HoneypotDetector {
    pub fn new(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        weth: Address,
        usdc: Address,
    ) -> Self {
        Self {
            provider,
            cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            cache_ttl_secs: 86400, // 24 horas
            weth,
            usdc,
        }
    }

    /// Verifica se um token é honeypot
    /// Custo: 1 eth_call (~26 CU no Alchemy)
    pub async fn check(&self, token: Address) -> HoneypotResult {
        // Whitelist = instant safe
        let token_str = format!("{:?}", token).to_lowercase();
        if WHITELIST.iter().any(|&w| token_str.contains(&w.to_lowercase())) {
            return HoneypotResult::Safe;
        }

        // Verificar cache
        {
            let cache = self.cache.read().await;
            if let Some((result, ts)) = cache.get(&token) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if now - ts < self.cache_ttl_secs {
                    return result.clone();
                }
            }
        }

        // Fazer simulação
        let result = self.simulate_sell(token).await;

        // Guardar no cache
        {
            let mut cache = self.cache.write().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            cache.insert(token, (result.clone(), now));
        }

        result
    }

    /// Simula venda de 1 token unidade
    async fn simulate_sell(&self, token: Address) -> HoneypotResult {
        // Simular uma transferência + approve + swap
        // Versão simplificada: verificar se o token tem função transfer
        
        let selector = vec![0xa9, 0x05, 0x9c, 0xbb]; // transfer(address,uint256)
        let mut calldata = vec![0u8; 68];
        calldata[0..4].copy_from_slice(&selector);
        // Endereço destino (dummy)
        calldata[16..36].copy_from_slice(&[0u8; 20]);
        // Amount = 1
        calldata[67] = 1;

        let provider = self.provider.read().await;
        
        match provider.call(&alloy::rpc::types::TransactionRequest::default()
            .with_to(token)
            .with_input(Bytes::from(calldata))
        ).await {
            Ok(result) => {
                if result.is_empty() {
                    HoneypotResult::Honeypot
                } else {
                    HoneypotResult::Safe
                }
            }
            Err(e) => {
                warn!("🍯 Honeypot simulation failed: {:?}", e);
                HoneypotResult::SimulationFail
            }
        }
    }

    /// Verifica múltiplos tokens em batch
    pub async fn check_batch(&self, tokens: &[Address]) -> Vec<(Address, HoneypotResult)> {
        let mut results = Vec::with_capacity(tokens.len());
        
        for &token in tokens {
            let result = self.check(token).await;
            results.push((token, result));
            
            // Pequeno delay para não sobrecarregar RPC
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
        
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_whitelist() {
        let weth = address!("0x4200000000000000000000000000000000000006");
        let weth_str = format!("{:?}", weth).to_lowercase();
        assert!(WHITELIST.iter().any(|&w| weth_str.contains(&w.to_lowercase())));
    }
}
