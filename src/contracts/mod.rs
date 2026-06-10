pub mod uniswap_v3;
pub mod aerodrome;

pub use uniswap_v3::{UniswapV3Pool, UniswapV3Factory, SwapEvent as UniswapV3Swap};
pub use aerodrome::{AerodromePool, AerodromeFactory, AerodromeRouter, SwapEvent as AerodromeSwap};

use alloy::primitives::{Address, U256, FixedBytes};
use serde::{Serialize, Deserialize};

/// Evento de Swap genérico normalizado
#[derive(Clone, Debug)]
pub struct NormalizedSwapEvent {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out: U256,
    pub block_number: u64,
    pub tx_hash: FixedBytes<32>,
    pub log_index: u64,
    pub sqrt_price_x96: Option<U256>,
    pub liquidity: Option<u128>,
    pub tick: Option<i32>,
    pub fee: u32,
    pub dex_type: DexType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DexType {
    UniswapV3,
    UniswapV2,
    Aerodrome,
    AerodromeStable,  // sAMM stable pools (x³y + xy³ = k)
    PancakeSwap,
}

/// Topic0 hashes para eventos de Swap
pub const UNISWAP_V3_SWAP_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0xc4, 0x20, 0x79, 0x94, 0x7c, 0x63, 0xc9, 0x73,
    0x05, 0xd3, 0x06, 0xc4, 0xb5, 0x70, 0xf0, 0x5c,
    0xa3, 0x09, 0x73, 0x1a, 0x5e, 0x61, 0x03, 0x0b,
    0x42, 0x79, 0x56, 0x7e, 0x94, 0x79, 0xe0, 0x5c,
]);

// CORREÇÃO 1: Topics correctos para a Base
// Sync event (V2/Aerodrome vAMM) - keccak256("Sync(uint112,uint112)")
pub const SYNC_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0x1c, 0x41, 0x1e, 0x9a, 0x96, 0xe0, 0x71, 0x24,
    0x1c, 0x2f, 0x21, 0xf7, 0x72, 0x6b, 0x17, 0xae,
    0x89, 0xe3, 0xca, 0xb4, 0xc7, 0x8b, 0xe5, 0x0e,
    0x06, 0x2b, 0x03, 0xa9, 0xff, 0xfb, 0xad, 0xd1,
]);

// Swap V3 (Uniswap V3 / Aerodrome Slipstream) - keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
pub const SWAP_V3_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0xc4, 0x20, 0x79, 0xf9, 0x4a, 0x63, 0x50, 0xd7,
    0xe6, 0x23, 0x5f, 0x29, 0x17, 0x49, 0x24, 0xf9,
    0x28, 0xcc, 0x2a, 0xc8, 0x18, 0xeb, 0x64, 0xfe,
    0xd8, 0x00, 0x4e, 0x11, 0x5f, 0xbc, 0xca, 0x67,
]);

// Swap Aerodrome vAMM - keccak256("Swap(address,uint256,uint256,uint256,uint256,address)")
pub const SWAP_AERO_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0xd7, 0x8a, 0xd9, 0x5f, 0xa4, 0x6c, 0x99, 0x4b,
    0x65, 0x51, 0xd0, 0xda, 0x85, 0xfc, 0x27, 0x5f,
    0xe6, 0x13, 0xce, 0x37, 0x65, 0x7f, 0xb8, 0xd5,
    0xe3, 0xd1, 0x30, 0x84, 0x01, 0x59, 0xd8, 0x22,
]);

// Manter compatibilidade com código antigo
pub const AERODROME_SWAP_TOPIC0: FixedBytes<32> = SWAP_AERO_TOPIC0;
pub const PANCAKESWAP_V3_SWAP_TOPIC0: FixedBytes<32> = SWAP_V3_TOPIC0;

/// Array de topic0s para subscrição - TODOS os eventos relevantes (3 topics)
pub const TOPICS_SWAP_EVENTS: [[u8; 32]; 3] = [
    SYNC_TOPIC0.0,      // Sync (V2/Aerodrome vAMM reserves)
    SWAP_V3_TOPIC0.0,   // Swap V3 (Uniswap V3 / Slipstream)
    SWAP_AERO_TOPIC0.0, // Swap Aerodrome vAMM
];

/// Converte um log bruto em evento normalizado
pub fn decode_swap_event(
    pool: Address,
    _topic0: FixedBytes<32>,
    data: &[u8],
    dex_type: DexType,
    block_number: u64,
    tx_hash: FixedBytes<32>,
    log_index: u64,
) -> Option<NormalizedSwapEvent> {
    match dex_type {
        DexType::UniswapV3 => uniswap_v3::decode_swap_event(pool, data, block_number, tx_hash, log_index),
        DexType::UniswapV2 => uniswap_v3::decode_swap_event(pool, data, block_number, tx_hash, log_index), // Uniswap V2 compatível
        DexType::Aerodrome => aerodrome::decode_swap_event(pool, data, block_number, tx_hash, log_index),
        DexType::PancakeSwap => uniswap_v3::decode_swap_event(pool, data, block_number, tx_hash, log_index), // PancakeSwap V3 usa mesmo ABI
        DexType::AerodromeStable => uniswap_v3::decode_swap_event(pool, data, block_number, tx_hash, log_index), // Slipstream/stable usa ABI compatível
    }
}

/// Verifica se o topic0 corresponde a um evento conhecido
/// CORREÇÃO 1: Agora reconhece Sync (V2), Swap V3, e Swap Aerodrome
pub fn classify_topic0(topic0: FixedBytes<32>) -> Option<DexType> {
    if topic0 == SWAP_V3_TOPIC0 {
        Some(DexType::UniswapV3) // Uniswap V3 / PancakeSwap V3 / Slipstream
    } else if topic0 == SWAP_AERO_TOPIC0 {
        Some(DexType::Aerodrome) // Aerodrome vAMM Swap
    } else if topic0 == SYNC_TOPIC0 {
        // Sync event pode ser V2 ou Aerodrome - vamos determinar pelo pool address depois
        Some(DexType::UniswapV2) // Por agora assume V2, refinamos depois
    } else {
        None
    }
}
