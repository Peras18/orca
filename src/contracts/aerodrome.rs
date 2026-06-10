use alloy::primitives::{Address, U256, FixedBytes, address};

/// Interface simplificada da Pool Aerodrome (stable e volatile)
#[derive(Clone, Debug)]
pub struct AerodromePool {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub is_stable: bool,
    pub reserve0: U256,
    pub reserve1: U256,
    pub block_timestamp_last: u32,
}

/// Interface da Factory Aerodrome
#[derive(Clone, Debug)]
pub struct AerodromeFactory {
    pub address: Address,
}

/// Endereço da Factory Aerodrome na Base Mainnet
pub const AERODROME_FACTORY: Address = address!("0x42024DAB8ED9bcE086865ACd50831A567Bb4258B");

impl AerodromeFactory {
    /// Endereço da Factory na Base mainnet
    pub const BASE_MAINNET: Address = AERODROME_FACTORY;

    pub fn new(address: Address) -> Self {
        Self { address }
    }

    /// Retorna o endereço da pool para um par de tokens
    pub fn get_pool(&self, token_a: Address, token_b: Address, is_stable: bool) -> Address {
        let (token0, token1) = if token_a < token_b {
            (token_a, token_b)
        } else {
            (token_b, token_a)
        };

        // Simplified - actual implementation uses CREATE2 with salt
        let mut data = Vec::with_capacity(41);
        data.extend_from_slice(token0.as_slice());
        data.extend_from_slice(token1.as_slice());
        data.push(if is_stable { 1 } else { 0 });
        let hasher = alloy::primitives::keccak256(&data);
        
        Address::from_slice(&hasher[12..32])
    }
}

/// Interface do Router Aerodrome
#[derive(Clone, Debug)]
pub struct AerodromeRouter {
    pub address: Address,
}

impl AerodromeRouter {
    /// Endereço do Router na Base mainnet
    pub const BASE_MAINNET: Address = Address::new([
        0xcF, 0x77, 0xa3, 0xDE, 0x79, 0x91, 0x1B, 0xdE,
        0xf3, 0x40, 0x97, 0x92, 0xd6, 0xC6, 0xdE, 0x56,
        0x2c, 0x47, 0x70, 0x5a,
    ]);
}

/// Evento Swap da Aerodrome
/// event Swap(
///     address indexed sender,
///     address indexed to,
///     uint256 amount0In,
///     uint256 amount1In,
///     uint256 amount0Out,
///     uint256 amount1Out
/// );
#[derive(Clone, Debug)]
pub struct SwapEvent {
    pub sender: Address,
    pub to: Address,
    pub amount0_in: U256,
    pub amount1_in: U256,
    pub amount0_out: U256,
    pub amount1_out: U256,
}

/// Decodifica dados do evento Swap da Aerodrome
pub fn decode_swap_event(
    pool: Address,
    data: &[u8],
    block_number: u64,
    tx_hash: FixedBytes<32>,
    log_index: u64,
) -> Option<super::NormalizedSwapEvent> {
    if data.len() < 128 {
        return None;
    }

    let amount0_in = U256::from_be_slice(&data[0..32]);
    let amount1_in = U256::from_be_slice(&data[32..64]);
    let amount0_out = U256::from_be_slice(&data[64..96]);
    let amount1_out = U256::from_be_slice(&data[96..128]);

    // Determinar direção do swap
    let (amount_in, amount_out, is_token0_in) = if amount0_in > U256::ZERO {
        (amount0_in, amount1_out, true)
    } else {
        (amount1_in, amount0_out, false)
    };

    // Placeholder - em produção, buscar tokens do cache de pools
    let token_in = Address::ZERO;
    let token_out = Address::ZERO;

    Some(super::NormalizedSwapEvent {
        pool,
        token_in,
        token_out,
        amount_in,
        amount_out,
        block_number,
        tx_hash,
        log_index,
        sqrt_price_x96: None,
        liquidity: None,
        tick: None,
        fee: if is_token0_in { 5 } else { 30 }, // 0.05% stable, 0.3% volatile
        dex_type: super::DexType::Aerodrome,
    })
}

/// Função de swap exata de entrada via Router
pub fn encode_swap_exact_tokens_for_tokens_params(
    amount_in: U256,
    amount_out_min: U256,
    routes: Vec<Route>,
    to: Address,
    deadline: U256,
) -> Vec<u8> {
    let mut params = Vec::with_capacity(256);
    
    // Selector: swapExactTokensForTokens(uint256,uint256,(address,address,bool)[],address,uint256)
    params.extend_from_slice(&[0x47, 0x2b, 0x43, 0xf3]);
    
    // ABI encoding
    params.extend_from_slice(&amount_in.to_be_bytes::<32>());
    params.extend_from_slice(&amount_out_min.to_be_bytes::<32>());
    
    // Offset para array de routes
    params.extend_from_slice(&U256::from(160).to_be_bytes::<32>());
    
    params.extend_from_slice(to.as_slice());
    params.extend_from_slice(&deadline.to_be_bytes::<32>());
    
    // Array de routes
    params.extend_from_slice(&U256::from(routes.len()).to_be_bytes::<32>());
    for route in routes {
        params.extend_from_slice(route.token_in.as_slice());
        params.extend_from_slice(route.token_out.as_slice());
        params.push(if route.stable { 1 } else { 0 });
        params.extend_from_slice(&[0u8; 31]); // padding
    }
    
    params
}

/// Rota para swap
#[derive(Clone, Debug)]
pub struct Route {
    pub token_in: Address,
    pub token_out: Address,
    pub stable: bool,
}

/// getReserves - reservas da pool
/// function getReserves() external view returns (uint256, uint256, uint32);
pub fn encode_get_reserves_call() -> Vec<u8> {
    // Selector: getReserves()
    vec![0x09, 0x02, 0xf1, 0xac]
}

pub fn decode_get_reserves_response(data: &[u8]) -> Option<(U256, U256, u32)> {
    if data.len() < 64 {
        return None;
    }
    
    let reserve0 = U256::from_be_slice(&data[0..32]);
    let reserve1 = U256::from_be_slice(&data[32..64]);
    let block_timestamp_last = u32::from_be_bytes(data[64..68].try_into().ok()?);
    
    Some((reserve0, reserve1, block_timestamp_last))
}

///stable - verifica se a pool é de tipo stable
/// function stable() external view returns (bool);
pub fn encode_stable_call() -> Vec<u8> {
    // Selector: stable()
    vec![0x1d, 0x5a, 0x7d, 0x03]
}
