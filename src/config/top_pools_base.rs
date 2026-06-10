//! 🏆 Top 5 Pools na Base para teste de stress reduzido
//!
//! Estas são as pools com maior volume/latividade na Base:
//! 1. WETH/USDC (Uniswap V3) - Maior pool da Base
//! 2. WETH/USDC (Aerodrome) - Alternativa importante
//! 3. WETH/DAI (Uniswap V3)
//! 4. cbETH/WETH (Uniswap V3) - LSD importante
//! 5. USDC/USDbC (Uniswap V3) - Stable pair

use alloy::primitives::{address, Address};

/// Top 5 pools na Base com maior atividade
pub const TOP_5_POOLS_BASE: [Address; 5] = [
    // 1. WETH/USDC Uniswap V3 (0.05%) - Pool #1 por volume
    address!("0xd0b53D9278572D2f58D11EA78acEA8e13FA9C72b"),
    
    // 2. WETH/USDC Aerodrome (Volatile) - Alternativa importante
    address!("0xB4885Bc63399bf5518b994c1d0C153334Ee57970"),
    
    // 3. WETH/DAI Uniswap V3 (0.3%) - Importante para triangular arb
    address!("0x6c561bDDd91bC965DB026fC7B1deC3B1259A1F25"),
    
    // 4. cbETH/WETH Uniswap V3 (0.05%) - LSD trading
    address!("0x106B446Cf11e28c50b557D8a9C43d0183D2B6E2e"),
    
    // 5. USDC/USDbC Uniswap V3 (0.01%) - Stable pair (menor volatilidade)
    address!("0x2223F9aC9E008982eA031325f6C0c4f96D7e9dC5"),
];

/// Configuração de teste com apenas 2 pools (mínimo para testar)
pub const TEST_2_POOLS: [Address; 2] = [
    address!("0xd0b53D9278572D2f58D11EA78acEA8e13FA9C72b"), // WETH/USDC UniV3
    address!("0xB4885Bc63399bf5518b994c1d0C153334Ee57970"), // WETH/USDC Aerodrome
];

/// Configuração de teste com apenas 1 pool (stress test mínimo)
pub const TEST_1_POOL: Address = address!("0xd0b53D9278572D2f58D11EA78acEA8e13FA9C72b");
