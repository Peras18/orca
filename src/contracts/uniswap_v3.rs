use alloy::primitives::{Address, FixedBytes, I256, U256};

/// Interface simplificada da Pool Uniswap V3
#[derive(Clone, Debug)]
pub struct UniswapV3Pool {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub sqrt_price_x96: U256,
    pub liquidity: u128,
    pub tick: i32,
}

/// Interface da Factory Uniswap V3
#[derive(Clone, Debug)]
pub struct UniswapV3Factory {
    pub address: Address,
}

impl UniswapV3Factory {
    // CORREÇÃO: este endereço estava errado desde o início (era um endereço
    // completamente diferente, não um typo) -- confirmado contra a
    // documentação oficial da Uniswap (docs.uniswap.org/contracts/v3/reference/deployments/base-deployments)
    // e validado on-chain via eth_getCode (endereço antigo devolvia "0x",
    // ou seja, nunca existiu contrato nenhum ali). Sem impacto funcional
    // real porque os 3 usos desta constante eram código morto (logs e uma
    // lista nunca consultada), mas corrigido por rigor.
    pub const ADDRESS: Address = Address::new([
        0x33, 0x12, 0x8a, 0x8f, 0xC1, 0x78, 0x69, 0x89, 0x7d, 0xcE, 0x68, 0xEd, 0x02, 0x6d, 0x69,
        0x46, 0x21, 0xf6, 0xFD, 0xfD,
    ]);

    /// Calcula o endereço da pool determinístico
    pub fn get_pool_address(&self, token_a: Address, token_b: Address, fee: u32) -> Address {
        let (token0, _token1) = if token_a < token_b {
            (token_a, token_b)
        } else {
            (token_b, token_a)
        };

        // Simplified pool address calculation (actual implementation uses CREATE2)
        let mut hasher = alloy::primitives::keccak256(&[]);
        hasher[0..20].copy_from_slice(token0.as_slice());
        hasher[20..24].copy_from_slice(&fee.to_be_bytes());
        Address::from_slice(&hasher[12..32])
    }
}

/// Evento Swap da Uniswap V3
/// event Swap(
///     address indexed sender,
///     address indexed recipient,
///     int256 amount0,
///     int256 amount1,
///     uint160 sqrtPriceX96,
///     uint128 liquidity,
///     int24 tick
/// );
#[derive(Clone, Debug)]
pub struct SwapEvent {
    pub sender: Address,
    pub recipient: Address,
    pub amount0: I256,
    pub amount1: I256,
    pub sqrt_price_x96: U256,
    pub liquidity: u128,
    pub tick: i32,
}

/// Decodifica dados do evento Swap
pub fn decode_swap_event(
    pool: Address,
    data: &[u8],
    block_number: u64,
    tx_hash: FixedBytes<32>,
    log_index: u64,
) -> Option<super::NormalizedSwapEvent> {
    // V3 Swap data layout (160 bytes total — sender & recipient are indexed topics):
    //   [0..32]    int256  amount0       (positive = token0 sold INTO pool)
    //   [32..64]   int256  amount1       (positive = token1 sold INTO pool)
    //   [64..96]   uint160 sqrtPriceX96  (padded to 32 bytes)
    //   [96..128]  uint128 liquidity     (right-aligned in 32-byte slot → actual in [112..128])
    //   [128..160] int24   tick          (right-aligned in 32-byte slot → actual in [156..160])
    if data.len() < 160 {
        return None;
    }

    let amount0 = I256::try_from_be_slice(&data[0..32]).unwrap_or_default();
    let amount1 = I256::try_from_be_slice(&data[32..64]).unwrap_or_default();
    let sqrt_price_x96 = U256::from_be_slice(&data[64..96]);
    // uint128 is ABI-encoded right-aligned in its 32-byte slot [96..128]
    let liquidity = u128::from_be_bytes(data[112..128].try_into().ok()?);
    // int24 is ABI-encoded right-aligned in its 32-byte slot [128..160]
    let tick = i32::from_be_bytes(data[156..160].try_into().ok()?);

    // Determine direction from signed amounts:
    // positive amount → tokens going INTO the pool (the token being sold)
    let (amount_in, amount_out) = if amount0 > I256::ZERO {
        // token0 is going IN, token1 is coming OUT
        (amount0.unsigned_abs(), amount1.unsigned_abs())
    } else {
        // token1 is going IN, token0 is coming OUT
        (amount1.unsigned_abs(), amount0.unsigned_abs())
    };

    Some(super::NormalizedSwapEvent {
        pool,
        token_in: Address::ZERO,
        token_out: Address::ZERO,
        amount_in,
        amount_out,
        block_number,
        tx_hash,
        log_index,
        sqrt_price_x96: Some(sqrt_price_x96),
        liquidity: Some(liquidity),
        tick: Some(tick),
        fee: 3000, // Default 0.3%
        dex_type: super::DexType::UniswapV3,
    })
}

/// Função de swap exata de entrada
pub fn encode_exact_input_single_params(
    token_in: Address,
    token_out: Address,
    fee: u32,
    recipient: Address,
    amount_in: U256,
    amount_out_minimum: U256,
    sqrt_price_limit_x96: U256,
) -> Vec<u8> {
    let mut params = Vec::with_capacity(256);

    // Selector: exactInputSingle((address,address,uint24,address,uint256,uint256,uint160))
    params.extend_from_slice(&[0x04, 0xe4, 0x5a, 0xaf]);

    // ABI encoding da struct
    params.extend_from_slice(token_in.as_slice());
    params.extend_from_slice(token_out.as_slice());
    params.extend_from_slice(&fee.to_be_bytes());
    params.extend_from_slice(&[0u8; 28]); // padding
    params.extend_from_slice(recipient.as_slice());
    params.extend_from_slice(&amount_in.to_be_bytes::<32>());
    params.extend_from_slice(&amount_out_minimum.to_be_bytes::<32>());
    params.extend_from_slice(&sqrt_price_limit_x96.to_be_bytes::<32>());

    params
}

/// Slot0 - estado atual da pool
/// function slot0() external view returns (
///     uint160 sqrtPriceX96,
///     int24 tick,
///     uint16 observationIndex,
///     uint16 observationCardinality,
///     uint16 observationCardinalityNext,
///     uint8 feeProtocol,
///     bool unlocked
/// );
pub fn encode_slot0_call() -> Vec<u8> {
    // Selector: slot0()
    vec![0x38, 0x5a, 0xe5, 0x85]
}

pub fn decode_slot0_response(data: &[u8]) -> Option<(U256, i32, bool)> {
    if data.len() < 64 {
        return None;
    }

    let sqrt_price_x96 = U256::from_be_slice(&data[0..32]);
    let tick = i32::from_be_bytes(data[32..36].try_into().ok()?);
    let unlocked = data[63] != 0;

    Some((sqrt_price_x96, tick, unlocked))
}

/// Liquidity - reserva atual
/// function liquidity() external view returns (uint128);
pub fn encode_liquidity_call() -> Vec<u8> {
    // Selector: liquidity()
    vec![0x1a, 0x69, 0x60, 0x98]
}
