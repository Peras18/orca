//! Dynamic Pool Discovery - O Olho de Sauron
//! 
//! Escuta eventos PoolCreated (Uniswap V3) e PairCreated (Aerodrome/PancakeSwap)
//! Adiciona novas pools automaticamente ao PoolRegistry se liquidez > $1,000

use alloy::primitives::{Address, B256, FixedBytes, address};
use alloy::providers::{RootProvider, Provider as AlloyProvider};
use alloy::rpc::types::eth::Filter;
use alloy::transports::BoxTransport;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, trace};

use crate::provider::{PoolRegistry, PoolInfo};
use crate::contracts::DexType;

// CORREÇÃO 3: Topics de factory corretos para discovery
/// Evento PoolCreated do Uniswap V3/Aerodrome Slipstream
/// topic0: 0x783cca1c1917ea5d459fa0efbab959b1b610bf5c2dfd9efb8b9452ed1d0cb0f
pub const TOPIC_UNISWAP_V3_POOL_CREATED: [u8; 32] = [
    0x78, 0x3c, 0xca, 0x1c, 0x19, 0x17, 0xea, 0x5d,
    0x45, 0x9f, 0xa0, 0xef, 0xba, 0xb9, 0x59, 0xb1,
    0xb6, 0x10, 0xbf, 0x5c, 0x2d, 0xfd, 0x9e, 0xfb,
    0x8b, 0x9a, 0x45, 0x2e, 0xd1, 0x0c, 0xb0, 0x0f,
];

/// Evento PairCreated do Uniswap V2/Aerodrome Classic
/// topic0: 0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9
pub const TOPIC_V2_PAIR_CREATED: [u8; 32] = [
    0x0d, 0x36, 0x48, 0xbd, 0x0f, 0x6b, 0xa8, 0x01,
    0x34, 0xa3, 0x3b, 0xa9, 0x27, 0x5a, 0xc5, 0x85,
    0xd9, 0xd3, 0x15, 0xf0, 0xad, 0x83, 0x55, 0xcd,
    0xde, 0xfd, 0xe3, 0x1a, 0xfa, 0x28, 0xd0, 0xe9,
];

/// Factories na Base Mainnet
pub const UNISWAP_V3_FACTORY: Address = Address::new([
    0x33, 0x12, 0x8a, 0x8f, 0xc1, 0x78, 0x69, 0x89,
    0x7d, 0xcE, 0x68, 0xEd, 0x02, 0x6d, 0x69, 0x46,
    0x21, 0xf6, 0xFD, 0xfD,
]);

/// Aerodrome V2 Factory na Base Mainnet
/// Endereço oficial: 0x42024DAb8ED9bcE086865ACd50831A567Bb4258B
pub const AERODROME_FACTORY: Address = address!("0x42024DAb8ED9bcE086865ACd50831A567Bb4258B");

/// Aerodrome Slipstream (V3) Factory na Base Mainnet
/// Endereço oficial: 0x5e79E80734891BA0907297920A0bA562Bf76632c
pub const AERODROME_SLIPSTREAM_FACTORY: Address = address!("0x5e79E80734891BA0907297920A0bA562Bf76632c");

pub const PANCAKESWAP_FACTORY: Address = Address::new([
    0x0b, 0xf8, 0xef, 0x9e, 0xbf, 0xfb, 0x03, 0x2b,
    0x1c, 0xa4, 0x8f, 0x45, 0x5c, 0xb7, 0xba, 0x4b,
    0x3b, 0x3f, 0x13, 0x8c,
]);

/// Scanner de factories para descoberta dinâmica de pools
pub struct FactoryScanner {
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    /// Cache de pools descobertas (address -> PoolInfo)
    discovered_pools: Arc<DashMap<Address, PoolInfo>>,
    /// Contador de pools descobertas
    discovery_count: Arc<RwLock<u64>>,
    /// Limite mínimo de liquidez ($1,000 USD)
    min_liquidity_usd: f64,
}

impl FactoryScanner {
    pub fn new(
        provider: Arc<RwLock<RootProvider<BoxTransport>>>,
        min_liquidity_usd: f64,
    ) -> Self {
        Self {
            provider,
            discovered_pools: Arc::new(DashMap::with_capacity(10000)),
            discovery_count: Arc::new(RwLock::new(0)),
            min_liquidity_usd,
        }
    }

    /// Inicia o scanner de factories em background
    pub async fn spawn(self: Arc<Self>) -> eyre::Result<()> {
        let provider = self.provider.clone();
        let discovered = self.discovered_pools.clone();
        let count = self.discovery_count.clone();
        let min_liq = self.min_liquidity_usd;

        tokio::spawn(async move {
            info!("👁️ OLHO DE SAURON ATIVO - Escutando factories...");
            info!("   Uniswap V3 Factory: {:?}", UNISWAP_V3_FACTORY);
            info!("   Aerodrome V2 Factory: {:?}", AERODROME_FACTORY);
            info!("   Aerodrome Slipstream Factory: {:?}", AERODROME_SLIPSTREAM_FACTORY);
            info!("   PancakeSwap Factory: {:?}", PANCAKESWAP_FACTORY);
            info!("   Limite mínimo de liquidez: ${:.0}", min_liq);

            loop {
                // CORREÇÃO 3: Criar filtro para eventos de criação de pools com todos os factories
                let filter = Filter::new()
                    .address(vec![
                        UNISWAP_V3_FACTORY, 
                        AERODROME_FACTORY, 
                        AERODROME_SLIPSTREAM_FACTORY,
                        PANCAKESWAP_FACTORY
                    ])
                    .event_signature(vec![
                        B256::new(TOPIC_UNISWAP_V3_POOL_CREATED),
                        B256::new(TOPIC_V2_PAIR_CREATED),
                    ]);

                let prov = provider.read().await;
                match prov.subscribe_logs(&filter).await {
                    Ok(mut stream) => {
                        info!("✅ FactoryScanner subscrito - aguardando novas pools...");

                        while let Ok(log) = stream.recv().await {
                            let topic0 = log.topics().first().map(|t| FixedBytes::new(t.0));
                            
                            if let Some(topic0) = topic0 {
                                if let Some(pool_info) = Self::decode_pool_created(&log, topic0) {
                                    let pool_addr = pool_info.address;
                                    
                                    // Verificar se já existe
                                    if discovered.contains_key(&pool_addr) {
                                        continue;
                                    }

                                    // 🎯 NOVA POOL DETETADA!
                                    *count.write().await += 1;
                                    let count_val = *count.read().await;
                                    
                                    info!(
                                        "🆕 [POOL-DISCOVERY #{}] Nova pool detetada! {:?} | {} | TVL: ${:.0}",
                                        count_val,
                                        pool_addr,
                                        match pool_info.dex_type {
                                            DexType::UniswapV3 => "Uniswap V3",
                                            DexType::UniswapV2 => "Uniswap V2",
                                            DexType::Aerodrome => "Aerodrome",
                                            DexType::AerodromeStable => "AerodromeStable",
                                            DexType::PancakeSwap => "PancakeSwap",
                                        },
                                        pool_info.tvl_usd
                                    );
                                    
                                    trace!(
                                        "   Tokens: {:?} <-> {:?} | Fee: {} bps",
                                        pool_info.token0, pool_info.token1, pool_info.fee / 100
                                    );

                                    // Adicionar ao cache
                                    discovered.insert(pool_addr, pool_info);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ FactoryScanner erro na subscrição: {}. Reconectando...", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        Ok(())
    }

    /// Decodifica evento de criação de pool
    fn decode_pool_created(log: &alloy::rpc::types::eth::Log, topic0: FixedBytes<32>) -> Option<PoolInfo> {
        let data = log.data().data.as_ref();
        let address = log.address();
        
        // CORREÇÃO 3: Determinar DEX type pela factory address
        let dex_type = if address == UNISWAP_V3_FACTORY {
            DexType::UniswapV3
        } else if address == AERODROME_FACTORY {
            DexType::Aerodrome
        } else if address == AERODROME_SLIPSTREAM_FACTORY {
            DexType::AerodromeStable // Slipstream usa stable math
        } else if address == PANCAKESWAP_FACTORY {
            DexType::PancakeSwap
        } else {
            return None;
        };

        // Verificar topic0
        let is_v3 = topic0.0 == TOPIC_UNISWAP_V3_POOL_CREATED;
        let is_v2 = topic0.0 == TOPIC_V2_PAIR_CREATED;

        if !is_v3 && !is_v2 {
            return None;
        }

        // Decodificar tokens e pool address
        // Uniswap V3: PoolCreated(address indexed token0, address indexed token1, uint24 fee, int24 tickSpacing, address pool)
        // V2: PairCreated(address indexed token0, address indexed token1, address pair, uint)
        
        if data.len() < 64 {
            return None;
        }

        // Extrair pool address (últimos 20 bytes do data para V3, topic3 para V2)
        let pool_address = if is_v3 && data.len() >= 20 {
            // Pool address está nos dados
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&data[data.len()-20..]);
            Address::new(addr_bytes)
        } else {
            // Para V2, tentar extrair de topics
            log.topics().get(3).map(|t| {
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(&t.0[12..32]);
                Address::new(addr_bytes)
            })?
        };

        // Extrair tokens de topics
        let token0 = log.topics().get(1).map(|t| {
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&t.0[12..32]);
            Address::new(addr_bytes)
        })?;

        let token1 = log.topics().get(2).map(|t| {
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&t.0[12..32]);
            Address::new(addr_bytes)
        })?;

        // Fee para V3 (decodificar de data)
        let fee = if is_v3 && data.len() >= 32 {
            let fee_bytes: [u8; 4] = data[0..4].try_into().unwrap_or([0, 0, 0, 5]);
            u32::from_be_bytes(fee_bytes)
        } else {
            3000 // 0.3% default para V2
        };

        Some(PoolInfo {
            address: pool_address,
            token0,
            token1,
            fee,
            dex_type,
            tvl_usd: 0.0, // Será atualizado depois
            priority: 0,
        })
    }

    /// Retorna todas as pools descobertas
    pub fn get_discovered_pools(&self) -> Vec<PoolInfo> {
        self.discovered_pools
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Adiciona pools descobertas ao registry
    pub fn merge_into_registry(&self, registry: &mut PoolRegistry) {
        let mut added = 0;
        
        for entry in self.discovered_pools.iter() {
            let pool = entry.value();
            if !registry.has_pool(&pool.address) && pool.tvl_usd >= self.min_liquidity_usd {
                registry.add_pool(pool.clone());
                added += 1;
            }
        }

        if added > 0 {
            info!("🔄 [MERGE] {} novas pools adicionadas ao registry", added);
        }
    }
}
