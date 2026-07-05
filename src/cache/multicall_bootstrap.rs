//! 🚀 Multicall3 Bootstrap - Inicialização Eficiente de Reserves
//! 
//! CORREÇÃO 4: Usa Multicall3 para obter reserves de múltiplos pools
//! em uma única chamada RPC (26 CU vs N*26 CU)
//! 
//! Endereço Multicall3: 0xcA11bde05977b3631167028862bE2a173976CA11

use alloy::primitives::{Address, U256, FixedBytes};
use alloy::sol_types::SolCall;
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

// Interface Multicall3 real -- sol! garante codificacao ABI correcta
// (a codificacao manual anterior tinha bugs: offset de array a zero e
// faltava a camada de offsets por-elemento exigida por arrays de structs
// com campos dinamicos, causando "execution reverted").
alloy::sol! {
    struct Call3 {
        address target;
        bool allowFailure;
        bytes callData;
    }
    struct Result3 {
        bool success;
        bytes returnData;
    }
    function aggregate3(Call3[] calls) external returns (Result3[] memory returnData);
}

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

        // CORREÇÃO: obter o bloco real ANTES do bootstrap -- sem isto, update_reserves_v2
        // usava bloco fixo 0, e is_stale() (que verifica current_block - last_update > 500)
        // marcava TODAS as pools do Multicall como obsoletas imediatamente, excluindo-as
        // do grafo apesar de terem reserves válidas.
        let current_block = {
            let provider = self.provider.read().await;
            provider.get_block_number().await.unwrap_or(0)
        };

        info!("🚀 [BOOTSTRAP] Inicializando {} pools via Multicall3 (bloco {})", pool_addresses.len(), current_block);
        
        let mut initialized = 0usize;
        let batches = pool_addresses.chunks(self.config.batch_size);
        
        for (batch_idx, batch) in batches.enumerate() {
            let current_block = current_block;
            debug!(
                "[BOOTSTRAP] Processando batch {}/{} ({} pools)",
                batch_idx + 1,
                (pool_addresses.len() + self.config.batch_size - 1) / self.config.batch_size,
                batch.len()
            );

            match self.process_batch(batch, current_block).await {
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
    async fn process_batch(&self, pool_addresses: &[Address], current_block: u64) -> Result<usize> {
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
                        state.update_reserves_v2(reserve0, reserve1, current_block);
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

    /// Constrói calldata para Multicall3.aggregate3 -- via sol! (ABI correcta garantida)
    fn build_aggregate3_calldata(&self, calls: &[crate::cache::pool_cache::Multicall3Call]) -> Result<Vec<u8>> {
        let sol_calls: Vec<Call3> = calls
            .iter()
            .map(|c| Call3 {
                target: c.target,
                allowFailure: c.allow_failure,
                callData: c.call_data.clone().into(),
            })
            .collect();
        let call = aggregate3Call { calls: sol_calls };
        Ok(call.abi_encode())
    }

    /// Executa chamada Multicall3
    async fn execute_multicall(&self, calldata: &[u8]) -> Result<Vec<Vec<u8>>> {
        let provider = self.provider.read().await;
        let tx = TransactionRequest::default()
            .to(self.multicall_address)
            .input(calldata.to_vec().into());
        let result = provider.call(&tx).await?;
        self.decode_aggregate3_result(&result)
    }

    /// Decodifica resultado de aggregate3 -- via sol! (ABI correcta garantida)
    fn decode_aggregate3_result(&self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        let decoded = aggregate3Call::abi_decode_returns(data, false)
            .map_err(|e| eyre::eyre!("Falha ao decodificar aggregate3: {}", e))?;
        Ok(decoded
            .returnData
            .into_iter()
            .map(|r| if r.success { r.returnData.to_vec() } else { Vec::new() })
            .collect())
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
