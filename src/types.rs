use alloy::primitives::{Address, U256, B256, FixedBytes, I256};
use std::hash::{Hash, Hasher};

pub use crate::contracts::DexType;

pub type Fixed64 = f64;

// CORREÇÃO 7: Topics globais para evitar erros de scope
/// Sync event (V2/Aerodrome vAMM) - keccak256("Sync(uint112,uint112)")
pub const SYNC_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0x1c, 0x41, 0x1e, 0x9a, 0x96, 0xe0, 0x71, 0x24,
    0x1c, 0x2f, 0x21, 0xf7, 0x72, 0x6b, 0x17, 0xae,
    0x89, 0xe3, 0xca, 0xb4, 0xc7, 0x8b, 0xe5, 0x0e,
    0x06, 0x2b, 0x03, 0xa9, 0xff, 0xfb, 0xad, 0xd1,
]);

/// Swap V3 (Uniswap V3 / Aerodrome Slipstream)
pub const SWAP_V3_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0xc4, 0x20, 0x79, 0xf9, 0x4a, 0x63, 0x50, 0xd7,
    0xe6, 0x23, 0x5f, 0x29, 0x17, 0x49, 0x24, 0xf9,
    0x28, 0xcc, 0x2a, 0xc8, 0x18, 0xeb, 0x64, 0xfe,
    0xd8, 0x00, 0x4e, 0x11, 0x5f, 0xbc, 0xca, 0x67,
]);

/// Swap Aerodrome vAMM
pub const SWAP_AERO_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0xd7, 0x8a, 0xd9, 0x5f, 0xa4, 0x6c, 0x99, 0x4b,
    0x65, 0x51, 0xd0, 0xda, 0x85, 0xfc, 0x27, 0x5f,
    0xe6, 0x13, 0xce, 0x37, 0x65, 0x7f, 0xb8, 0xd5,
    0xe3, 0xd1, 0x30, 0x84, 0x01, 0x59, 0xd8, 0x22,
]);

/// Factory events para discovery
/// Uniswap V2 PairCreated
pub const PAIR_CREATED_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0x0d, 0x36, 0x48, 0xbd, 0x0f, 0x6b, 0xa8, 0x01,
    0x34, 0xa3, 0x3b, 0xa9, 0x27, 0x5a, 0xc5, 0x85,
    0xd9, 0xd3, 0x15, 0xf0, 0xad, 0x83, 0x55, 0xcd,
    0xde, 0xfd, 0xe3, 0x1a, 0xfa, 0x28, 0xd0, 0xe9,
]);

/// Uniswap V3 PoolCreated
pub const POOL_CREATED_TOPIC0: FixedBytes<32> = FixedBytes::new([
    0x78, 0x3c, 0xca, 0x1c, 0x19, 0x17, 0xea, 0x5d,
    0x45, 0x9f, 0xa0, 0xef, 0xba, 0xb9, 0x59, 0xb1,
    0xb6, 0x10, 0xbf, 0x5c, 0x2d, 0xfd, 0x9e, 0xfb,
    0x8b, 0x9a, 0x45, 0x2e, 0xd1, 0x0c, 0xb0, 0x0f,
]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pool {
    pub address: Address,
    pub token_a: Address,
    pub token_b: Address,
    pub fee: u32,
    pub reserve_a: U256,
    pub reserve_b: U256,
    pub dex_type: DexType,
}

#[derive(Clone, Copy, Debug)]
pub struct PriceUpdate {
    pub pool: Address,
    pub sqrt_price_x96: U256,
    pub liquidity: U128,
    pub tick: i32,
    pub timestamp: u64,
}

pub type U128 = alloy::primitives::U128;

#[derive(Clone, Debug)]
pub struct ArbitragePath {
    pub hops: Vec<Hop>,
    pub input_token: Address,
    pub optimal_input: U256,
    pub expected_profit: U256,
    pub profit_ratio: Fixed64,
}

#[derive(Clone, Copy, Debug)]
pub struct Hop {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub fee: u32,
    pub dex_type: DexType,
}

#[derive(Clone, Debug)]
pub struct MempoolTx {
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub data: Vec<u8>,
    pub value: U256,
    pub gas_price: U256,
    pub gas_limit: u64,
    pub nonce: u64,
}

#[derive(Clone, Debug)]
pub struct SimulationResult {
    pub success: bool,
    pub gas_used: u64,
    pub output_amount: U256,
    pub state_diff: StateDiff,
    pub is_honeypot: bool,
}

#[derive(Clone, Debug, Default)]
pub struct StateDiff {
    pub storage_changes: Vec<StorageChange>,
    pub balance_changes: Vec<BalanceChange>,
}

#[derive(Clone, Debug)]
pub struct StorageChange {
    pub address: Address,
    pub slot: FixedBytes<32>,
    pub old_value: FixedBytes<32>,
    pub new_value: FixedBytes<32>,
}

#[derive(Clone, Debug)]
pub struct BalanceChange {
    pub address: Address,
    pub token: Address,
    pub delta: I256,
}

#[derive(Clone, Debug)]
pub struct Opportunity {
    pub kind: OpportunityKind,
    pub trigger_tx: B256,
    pub path: ArbitragePath,
    pub confidence: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpportunityKind {
    Backrun,
    Sandwich,
    BugBounty,
}

pub struct FastHash<T>(pub T);

impl<T: Hash> Hash for FastHash<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: PartialEq> PartialEq for FastHash<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl<T: Eq> Eq for FastHash<T> {}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct AlignedPoolCache {
    pub address: [u8; 20],
    pub reserve_a: [u8; 32],
    pub reserve_b: [u8; 32],
    pub fee: u32,
    pub _padding: [u8; 28],
}

impl AlignedPoolCache {
    pub const fn size() -> usize {
        std::mem::size_of::<Self>()
    }
}

// ═══════════════════════════════════════════════════════════
// TIPOS PARA JITO-STYLE BUNDLE BUILDER
// ═══════════════════════════════════════════════════════════

/// 🐋 Swap de Baleia Pendente
#[derive(Clone, Debug)]
pub struct PendingWhaleSwap {
    pub tx_hash: FixedBytes<32>,
    pub pool_address: Address,
    pub from: Address,
    pub value_eth: f64,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub estimated_price_impact: f64,
    pub detected_at: std::time::Instant,
    pub deadline: std::time::Instant,
}

/// 📦 Transação de Bundle
#[derive(Clone, Debug)]
pub struct BundleTransaction {
    pub tx_hash: FixedBytes<32>,
    pub to: Address,
    pub data: alloy::primitives::Bytes,
    pub value: U256,
    pub gas_price: U256,
    pub priority: u8,
    pub is_whale: bool,
}

/// 💰 Oportunidade de Arbitragem
#[derive(Clone, Debug)]
pub struct ArbitrageOpportunity {
    pub path: Vec<Address>,
    pub target_pool: Address,
    pub amount_in: U256,
    pub expected_profit_usd: f64,
    pub estimated_gas_cost: u64,
    pub calldata: alloy::primitives::Bytes,
    pub deadline: std::time::Instant,
}

/// 📊 Reservas de Pool
#[derive(Clone, Debug, Copy, Default)]
pub struct PoolReserves {
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub fee: u32,
}
