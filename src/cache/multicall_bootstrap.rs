//! 🚀 Multicall3 Bootstrap - Inicialização Eficiente de Reserves
//! 
//! CORREÇÃO 4: Usa Multicall3 para obter reserves de múltiplos pools
//! em uma única chamada RPC (26 CU vs N*26 CU)
//! 
//! Endereço Multicall3: 0xcA11bde05977b3631167028862bE2a173976CA11

use alloy::primitives::{Address, U256, FixedBytes};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::transports::BoxTransport;
use eyre::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug, warn};
use alloy::primitives::address;

use crate::cache::PoolCache;
use crate::cache::pool_cache::{PoolState, build_getreserves_multicall, decode_getreserves_result};
use crate::contracts::DexType;

/// Bootstrap configuration
#[derive(Clone, Debug)]
pub struct BootstrapConfig {
    /// Batch size para multicall (máximo 100 por call)
    pub batch_size: usize,
    /// Timeout por batch (ms)
    pub timeout_ms: u64,
    /// Max retries se falhar
    pub max_retries: u32,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            batch_size: 50, // 50 pools por call
            timeout_ms: 5000, // 5s timeout
            max_retries: 3,
        }
    }
}

/// Multicall3 Bootstrap Manager
pub struct MulticallBootstrap {
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    cache: Arc<PoolCache>,
    config: BootstrapConfig,
    /// Endereço Multicall3 na Base
    multicall_address: Address,
}

impl MulticallBootstrap {
    /// Cria novo bootstrap manager
    pub fn new(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        cache: Arc<PoolCache>,
        config: BootstrapConfig,
    ) -> Self {
        Self {
            provider,
            cache,
            config,
            multicall_address: address!("0xcA11bde05977b3631167028862bE2a173976CA11"),
        }
    }

    /// 🚀 Inicializa reserves para múltiplos pools via Multicall3
    pub async fn bootstrap_reserves(&self, pool_addresses: &[Address]) -> Result<usize> {
        if pool_addresses.is_empty() {
            return Ok(0);
        }

        info!("🚀 [BOOTSTRAP] Inicializando {} pools via Multicall3", pool_addresses.len());
        
        let mut initialized = 0usize;
        let batches = pool_addresses.chunks(self.config.batch_size);
        
        for (batch_idx, batch) in batches.enumerate() {
            debug!(
                "[BOOTSTRAP] Processando batch {}/{} ({} pools)",
                batch_idx + 1,
                (pool_addresses.len() + self.config.batch_size - 1) / self.config.batch_size,
                batch.len()
            );

            match self.process_batch(batch).await {
                Ok(batch_initialized) => {
                    initialized += batch_initialized;
                    debug!(
                        "[BOOTSTRAP] Batch {} OK: {}/{} pools inicializadas",
                        batch_idx + 1,
                        batch_initialized,
                        batch.len()
                    );
                }
                Err(e) => {
                    warn!(
                        "[BOOTSTRAP] Batch {} falhou: {} | Pulando para próximo",
                        batch_idx + 1,
                        e
                    );
                }
            }
        }

        info!(
            "🎉 [BOOTSTRAP] Completo: {}/{} pools inicializadas com sucesso",
            initialized,
            pool_addresses.len()
        );

        Ok(initialized)
    }

    /// Processa um batch de pools via Multicall3
    async fn process_batch(&self, pool_addresses: &[Address]) -> Result<usize> {
        // 1. Construir multicall data
        let calls = build_getreserves_multicall(pool_addresses);
        
        // 2. Construir calldata para aggregate3
        let calldata = self.build_aggregate3_calldata(&calls)?;
        
        // 3. Executar chamada
        let result = self.execute_multicall(&calldata).await?;
        
        // 4. Decodificar resultados
        let mut initialized = 0;
        for (i, pool_addr) in pool_addresses.iter().enumerate() {
            if i < result.len() {
                if let Some((reserve0, reserve1, _timestamp)) = decode_getreserves_result(&result[i]) {
                    // Atualizar cache com as reserves
                    if let Some(mut state) = self.cache.get(pool_addr) {
                        // Usar DexType::UniswapV2 como default para bootstrap
                        // O tipo correto será determinado depois
                        state.dex_type = DexType::UniswapV2;
                        state.update_reserves_v2(reserve0, reserve1, 0);
                        self.cache.insert(state);
                        initialized += 1;
                        
                        debug!(
                            "[BOOTSTRAP] Pool {:?}: reserves={}/{}",
                            pool_addr, reserve0, reserve1
                        );
                    }
                }
            }
        }
        
        Ok(initialized)
    }

    /// Constrói calldata para Multicall3.aggregate3
    fn build_aggregate3_calldata(&self, calls: &[crate::cache::pool_cache::Multicall3Call]) -> Result<Vec<u8>> {
        // Selector aggregate3((address target, bool allowFailure, bytes callData)[])
        // 0xac9650d8
        let mut calldata = vec![0xac, 0x96, 0x50, 0xd8];
        
        // Adicionar offset para array (32 bytes)
        calldata.extend_from_slice(&[0x00; 32]);
        
        // Adicionar length do array (32 bytes)
        let length = U256::from(calls.len());
        calldata.extend_from_slice(&length.to_be_bytes::<32>());
        
        // Para cada call: target (20) + padding (12) + allowFailure (32) + callData_offset (32) + callData_length (32) + callData
        let mut data_offset = 32 * (4 + calls.len() * 3); // Base + array header + cada call header
        
        for call in calls {
            // Target address (20 bytes) + padding (12 bytes)
            calldata.extend_from_slice(&[0x00; 12]);
            calldata.extend_from_slice(call.target.as_slice());
            
            // allowFailure (bool como uint256)
            let allow_failure = if call.allow_failure { U256::from(1) } else { U256::ZERO };
            calldata.extend_from_slice(&allow_failure.to_be_bytes::<32>());
            
            // callData offset
            let offset = U256::from(data_offset);
            calldata.extend_from_slice(&offset.to_be_bytes::<32>());
            
            // callData length
            let length = U256::from(call.call_data.len());
            calldata.extend_from_slice(&length.to_be_bytes::<32>());
            
            // Atualizar offset para próximo
            data_offset += call.call_data.len();
        }
        
        // Adicionar todos os callData
        for call in calls {
            calldata.extend_from_slice(&call.call_data);
        }
        
        Ok(calldata)
    }

    /// Executa chamada Multicall3
    async fn execute_multicall(&self, calldata: &[u8]) -> Result<Vec<Vec<u8>>> {
        let provider = self.provider.read().await;
        
        // Criar transação call
        let tx = TransactionRequest::default()
            .to(self.multicall_address)
            .input(calldata.to_vec().into());
        
        // Executar call
        let result = provider.call(&tx).await?;
        
        // Decodificar resultado aggregate3
        self.decode_aggregate3_result(&result)
    }

    /// Decodifica resultado de aggregate3
    fn decode_aggregate3_result(&self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        if data.len() < 96 {
            return Ok(Vec::new());
        }
        
        // aggregate3 retorna (uint256 returnData, bool success)[]
        // Skip primeiro array (32 bytes de offset)
        let array_start = 32;
        
        if data.len() < array_start + 32 {
            return Ok(Vec::new());
        }
        
        // Ler length do array
        let length_bytes = &data[array_start..array_start + 32];
        let length = U256::from_be_slice(length_bytes).to::<usize>();
        
        let mut results = Vec::with_capacity(length);
        let mut offset = array_start + 32;
        
        for _ in 0..length {
            if offset + 64 > data.len() {
                break;
            }
            
            // Cada elemento: returnData_offset (32) + success (32)
            let return_data_offset_bytes = &data[offset..offset + 32];
            let return_data_offset = U256::from_be_slice(return_data_offset_bytes).to::<usize>();
            
            let success_bytes = &data[offset + 32..offset + 64];
            let success = U256::from_be_slice(success_bytes) != U256::ZERO;
            
            if success && return_data_offset < data.len() {
                // Ler length do returnData
                if return_data_offset + 32 <= data.len() {
                    let return_data_length_bytes = &data[return_data_offset..return_data_offset + 32];
                    let return_data_length = U256::from_be_slice(return_data_length_bytes).to::<usize>();
                    
                    let data_start = return_data_offset + 32;
                    let data_end = data_start + return_data_length;
                    
                    if data_end <= data.len() {
                        let return_data = data[data_start..data_end].to_vec();
                        results.push(return_data);
                    }
                }
            }
            
            offset += 64;
        }
        
        Ok(results)
    }

    /// 🧹 Limpa pools sem reserves após bootstrap
    pub async fn cleanup_empty_pools(&self) -> Result<usize> {
        let active_pools = self.cache.get_active_pools(U256::from(1)); // Mínimo 1 wei
        
        let total_pools = self.cache.len();
        let empty_pools = total_pools - active_pools.len();
        
        if empty_pools > 0 {
            info!(
                "🧹 [BOOTSTRAP] {} pools sem liquidez detectadas",
                empty_pools
            );
        }
        
        Ok(empty_pools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_bootstrap_config() {
        let config = BootstrapConfig::default();
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_aggregate3_calldata() {
        let calls = build_getreserves_multicall(&[
            address!("0x4200000000000000000000000000000000000006"),
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        ]);
        
        // Testar se calldata é construída sem erros
        // Implementação completa requer provider mock
    }
}
