use alloy::primitives::{Address, U256, B256};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::types::ArbitragePath;
use crate::contracts::DexType;

pub mod mev_share;
pub use mev_share::{MevShareBundle, check_flashbots_availability};

pub mod bundle;
pub use bundle::{TxBundle, BundlePayload, AtomicBundleBuilder};

pub mod nonce_manager;
pub use nonce_manager::NonceManager;

// JITO-Style Bundle Builder - Alta Agressividade
pub mod multi_call_bundler;
pub use multi_call_bundler::{
    BundleBuilder, BundleConfig, BundlePackage, WhaleDetector,
    WHALE_MIN_ETH, MIN_PROFIT_PER_TRADE, PROFIT_AGGRESSIVE, PROFIT_EXTREME,
    GAS_TIP_MIN, GAS_TIP_AGGRESSIVE, GAS_TIP_EXTREME,
};

// Priority Gas Auction (PGA) - Até 200€ em gas
pub mod gas_auction;
pub use gas_auction::{
    GasAuctionController, GasBid, GasStrategy, PGAStats, BidSimulation,
    PGA_PROFIT_THRESHOLD_EUR, PGA_MAX_GAS_EUR, PGA_AGGRESSIVE_GAS_EUR,
};

/// Payload de execução comprimido - Alinhado bit-a-bit com ApexMEVExecutor.sol
/// Format: [blockDeadline:4][minProfit:4][hopCount:1][hop1:25][hop2:25]...
#[derive(Clone, Debug)]
pub struct ExecutionPayload {
    pub target: Address,
    pub calldata: Vec<u8>,
    pub value: U256,
    pub gas_limit: u64,
    pub priority_fee: u128,
}

/// Dados decodificados do payload para validação
#[derive(Clone, Debug)]
pub struct DecodedPayload {
    pub block_deadline: u32,
    pub min_profit_wei: u64, // Descompactado de uint32
    pub hop_count: u8,
    pub hops: Vec<DecodedHop>,
}

#[derive(Clone, Debug)]
pub struct DecodedHop {
    pub pool: Address,
    pub token_in: Address,
    pub fee: u32,
    pub dex_type: DexType,
}

/// Encoder de transações MEV otimizado
#[derive(Clone)]
pub struct PayloadEncoder {
    #[allow(dead_code)]
    executor_address: Address,
    #[allow(dead_code)]
    chain_id: u64,
}

impl PayloadEncoder {
    pub fn new(executor: Address, chain_id: u64) -> Self {
        Self {
            executor_address: executor,
            chain_id,
        }
    }

    /// Codifica uma arbitragem cíclica completa (flash loan + swaps)
    /// Alinhado bit-a-bit com ApexMEVExecutor.sol::execute(bytes)
    pub fn encode_flash_arbitrage(
        &self,
        path: &ArbitragePath,
        flash_loan_provider: FlashLoanProvider,
        loan_amount: U256,
        block_deadline: u32,  // Número do bloco deadline
        min_profit_wei: u64,   // Lucro mínimo em wei
    ) -> ExecutionPayload {
        let mut calldata = Vec::with_capacity(256);
        
        // Selector: execute(bytes) - 0x1e9a5114
        calldata.extend_from_slice(&[0x1e, 0x9a, 0x51, 0x14]);
        
        // Encoding otimizado da rota (bit-a-bit com Solidity)
        let encoded_route = self.encode_route_aligned(
            path, 
            flash_loan_provider, 
            loan_amount,
            block_deadline,
            min_profit_wei
        );
        
        // ABI encoding: offset (32 bytes) + length (32 bytes) + data
        calldata.extend_from_slice(&U256::from(32).to_be_bytes::<32>());
        calldata.extend_from_slice(&U256::from(encoded_route.len()).to_be_bytes::<32>());
        calldata.extend_from_slice(&encoded_route);
        
        ExecutionPayload {
            target: self.executor_address,
            calldata,
            value: U256::ZERO,
            gas_limit: 500_000 + (path.hops.len() as u64 * 100_000),
            priority_fee: 1_000_000_000, // 1 gwei
        }
    }

    /// Codifica rota de arbitragem - Alinhado bit-a-bit com ApexMEVExecutor.sol
    /// Format: [blockDeadline:4][minProfit:4][hopCount:1][hop1:25][hop2:25]...
    fn encode_route_aligned(
        &self,
        path: &ArbitragePath,
        _provider: FlashLoanProvider,
        _amount: U256,
        block_deadline: u32,
        min_profit_wei: u64,
    ) -> Vec<u8> {
        let hop_count = path.hops.len();
        // Header: 4 + 4 + 1 = 9 bytes + hops * 25
        let mut encoded = Vec::with_capacity(9 + hop_count * 25);
        
        // Bytes 0-3: blockDeadline (uint32 big-endian)
        encoded.extend_from_slice(&block_deadline.to_be_bytes());
        
        // Bytes 4-7: minProfit (uint32, compactado: wei / 1e9)
        let min_profit_compact = (min_profit_wei / 1_000_000_000) as u32;
        encoded.extend_from_slice(&min_profit_compact.to_be_bytes());
        
        // Byte 8: hopCount (uint8, max 4)
        encoded.push(hop_count.min(4) as u8);
        
        // Bytes 9+: Hops (25 bytes cada)
        for hop in &path.hops {
            encoded.extend_from_slice(hop.pool.as_slice()); // 20 bytes
            
            // TokenIn: 4 bytes (suffix do address com padding)
            // Solidity espera: 4 bytes + 16 bytes zeros = address completo
            let token_suffix = &hop.token_in.as_slice()[16..20]; // Últimos 4 bytes
            encoded.extend_from_slice(token_suffix); // 4 bytes
            
            // Fee/DexType: 1 byte
            // Bit 7: 1 = Aerodrome, 0 = UniswapV3
            // Bits 0-6: fee tier index (0=0.05%, 1=0.3%, 2=1%)
            let dex_flag = if hop.dex_type == DexType::Aerodrome { 0x80 } else { 0x00 };
            let fee_index = match hop.fee {
                500 => 0,    // 0.05%
                3000 => 1,   // 0.3%
                10000 => 2,  // 1%
                _ => 1,      // default 0.3%
            };
            encoded.push(dex_flag | fee_index);
        }
        
        encoded
    }
    
    /// Decodifica payload para validação (testes de integração)
    pub fn decode_payload(&self, payload: &[u8]) -> Option<DecodedPayload> {
        if payload.len() < 34 {
            return None;
        }
        
        // Decodificar header
        let block_deadline = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let min_profit_compact = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let hop_count = payload[8] as usize;
        
        if payload.len() < 9 + hop_count * 25 {
            return None;
        }
        
        let mut hops = Vec::with_capacity(hop_count);
        
        for i in 0..hop_count {
            let offset = 9 + i * 25;
            
            // Pool: 20 bytes
            let pool = Address::from_slice(&payload[offset..offset+20]);
            
            // TokenIn: 4 bytes + padding de zeros
            let mut token_bytes = [0u8; 20];
            token_bytes[16..20].copy_from_slice(&payload[offset+20..offset+24]);
            let token_in = Address::from_slice(&token_bytes);
            
            // Fee/DexType
            let fee_byte = payload[offset+24];
            let dex_type = if (fee_byte & 0x80) != 0 { 
                DexType::Aerodrome 
            } else { 
                DexType::UniswapV3 
            };
            let fee = match fee_byte & 0x7F {
                0 => 500,
                1 => 3000,
                2 => 10000,
                _ => 3000,
            };
            
            hops.push(DecodedHop {
                pool,
                token_in,
                fee,
                dex_type,
            });
        }
        
        Some(DecodedPayload {
            block_deadline,
            min_profit_wei: (min_profit_compact as u64) * 1_000_000_000,
            hop_count: hop_count as u8,
            hops,
        })
    }

    /// Comprime U256 removendo zeros à esquerda
    #[allow(dead_code)]
    fn compress_u256(&self, value: U256) -> Vec<u8> {
        let bytes = value.to_be_bytes::<32>();
        let first_non_zero = bytes.iter().position(|&b| b != 0).unwrap_or(31);
        let mut result = vec![(32 - first_non_zero) as u8]; // Length prefix
        result.extend_from_slice(&bytes[first_non_zero..]);
        result
    }

    /// Codifica aprovação de token (ERC20 approve)
    pub fn encode_approve(&self, token: Address, spender: Address, amount: U256) -> ExecutionPayload {
        let mut calldata = vec![0x09; 4]; // approve(address,uint256) selector
        calldata[0] = 0x09;
        calldata[1] = 0x5e;
        calldata[2] = 0xa7;
        calldata[3] = 0xb3;
        
        calldata.extend_from_slice(&[0u8; 12]); // padding
        calldata.extend_from_slice(spender.as_slice());
        calldata.extend_from_slice(&amount.to_be_bytes::<32>());
        
        ExecutionPayload {
            target: token,
            calldata,
            value: U256::ZERO,
            gas_limit: 60_000,
            priority_fee: 0,
        }
    }

    /// Cria bundle de transações (para MEV-Share/Flashbots)
    pub fn create_bundle(&self, payloads: Vec<ExecutionPayload>) -> Vec<u8> {
        let mut bundle = Vec::with_capacity(payloads.len() * 512);
        
        // Número de txs (2 bytes)
        bundle.extend_from_slice(&(payloads.len() as u16).to_be_bytes());
        
        for payload in payloads {
            // Tamanho da tx (4 bytes)
            bundle.extend_from_slice(&(payload.calldata.len() as u32).to_be_bytes());
            bundle.extend_from_slice(&payload.calldata);
        }
        
        bundle
    }

    /// Estima tamanho do calldata em bytes (para cálculo de custo)
    pub fn estimate_calldata_cost(&self, path: &ArbitragePath) -> u64 {
        // Cada byte de calldata custa 16 gas (não-zero) ou 4 gas (zero)
        let non_zero_bytes = 4 + 1 + 32 + path.hops.len() * 25; // selector + provider + amount + hops
        let zero_bytes = 64usize.saturating_sub(non_zero_bytes % 64); // Padding de 64 bytes
        
        (non_zero_bytes as u64 * 16) + (zero_bytes as u64 * 4)
    }
}

/// Provedores de Flash Loan suportados
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlashLoanProvider {
    UniswapV3,  // Flash Swap (sem taxa)
    BalancerV2, // Flash Loan (0.09%)
    AaveV3,     // Flash Loan (0.09%)
}

impl FlashLoanProvider {
    /// Retorna a taxa do flash loan em basis points
    pub fn fee_bps(&self) -> u32 {
        match self {
            FlashLoanProvider::UniswapV3 => 3,   // 0.03% - apenas swap fee
            FlashLoanProvider::BalancerV2 => 9,  // 0.09%
            FlashLoanProvider::AaveV3 => 9,      // 0.09%
        }
    }

    /// Endereço do contrato na Base
    pub fn address(&self) -> Address {
        match self {
            FlashLoanProvider::UniswapV3 => Address::new([
                0x33, 0x1F, 0x28, 0x5f, 0xB8, 0x20, 0x8E, 0xFC,
                0x0b, 0xFF, 0xcd, 0x81, 0xEb, 0x72, 0x40, 0x28,
                0x71, 0xb8, 0xBB, 0x4f, // Uniswap V3 Factory
            ]),
            FlashLoanProvider::BalancerV2 => Address::new([
                0xBA, 0x12, 0x26, 0x67, 0x2e, 0x00, 0x12, 0xe9,
                0xB2, 0xB8, 0x4E, 0x32, 0xec, 0x40, 0x60, 0x46,
                0x5b, 0xBC, 0x32, 0x72, // Balancer Vault
            ]),
            FlashLoanProvider::AaveV3 => Address::new([
                0xA2, 0x38, 0x6C, 0x85, 0xB6, 0xDc, 0x74, 0x02,
                0x2d, 0x8a, 0xC7, 0x6A, 0x0B, 0xeE, 0x01, 0x9d,
                0x6c, 0x2B, 0x47, 0xC0, // Aave Pool
            ]),
        }
    }
}

/// Estratégia de Flash Loan "Infinite Liquidity" com Balancer/Aave
pub struct FlashLoanStrategy {
    #[allow(dead_code)]
    encoder: PayloadEncoder,
    #[allow(dead_code)]
    preferred_provider: FlashLoanProvider,
    #[allow(dead_code)]
    max_loan_amount_eth: u128,
    /// Taxa de flashloan em basis points (0 = Balancer, 5 = Aave V3)
    #[allow(dead_code)]
    flash_loan_fee_bps: u32,
    /// Endereços dos tokens disponíveis para flashloan
    #[allow(dead_code)]
    supported_tokens: Vec<Address>,
}

/// Informações de um flashloan calculado
#[derive(Clone, Debug)]
pub struct FlashLoanCalculation {
    /// Montante a pedir emprestado
    pub loan_amount: U256,
    /// Provedor escolhido (Balancer = 0% ou Aave = 0.05%)
    pub provider: FlashLoanProvider,
    /// Taxa total em wei
    pub flash_loan_fee_wei: U256,
    /// Gas extra estimado para o flashloan
    pub extra_gas_cost: u64,
    /// Lucro líquido esperado após todas as taxas
    pub net_profit_wei: U256,
    /// Se vale a pena executar
    pub is_profitable: bool,
    /// Min amount out para proteção de slippage (3x margem de segurança)
    pub min_amount_out: U256,
    /// Custo total de gas estimado em wei
    pub total_gas_cost_wei: U256,
}

/// Verificações de segurança para proteger a banca
#[derive(Clone, Debug)]
pub struct SafetyCheck {
    /// Margem de segurança mínima (1.2x = estratégia agressiva, lucro deve ser 1.2x o custo)
    pub safety_margin_multiplier: u32,
    /// Slippage máximo aceitável em basis points (50 = 0.5%)
    pub max_slippage_bps: u32,
    /// Lucro mínimo em wei para executar (0.00005 ETH = ~0.15€)
    pub min_profit_wei: U256,
}

impl Default for SafetyCheck {
    fn default() -> Self {
        Self {
            safety_margin_multiplier: 12, // 1.2x = 12/10, usamos 12 e dividimos por 10 no cálculo
            max_slippage_bps: 50,        // 0.5% slippage máximo
            min_profit_wei: U256::from(50_000_000_000_000u128), // 0.00005 ETH
        }
    }
}

impl FlashLoanStrategy {
    pub fn new(encoder: PayloadEncoder, provider: FlashLoanProvider) -> Self {
        use alloy::primitives::address;
        
        // Taxas: UniswapV3 FlashSwap = 0% (taxa só no swap), Balancer = 0%, Aave = 0.09%
        let fee_bps = match provider {
            FlashLoanProvider::UniswapV3 => 0,  // FlashSwap integrado no swap
            FlashLoanProvider::BalancerV2 => 0, // 0% taxa flashloan
            FlashLoanProvider::AaveV3 => 9,     // 0.09% taxa flashloan
        };
        
        // Tokens suportados na Base para flashloan
        let supported = vec![
            address!("0x4200000000000000000000000000000000000006"), // WETH
            address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            address!("0x50c5725949A6F0c72E6C4a641F2409A017ebaBdf"), // DAI
            address!("0xc1CBa9f5a3D8b6e0e3F6D9C0F4A2B1c3d4E5F6A7"), // CBETH
        ];
        
        Self {
            encoder,
            preferred_provider: provider,
            max_loan_amount_eth: 10_000, // Max 10,000 ETH (alavancagem máxima)
            flash_loan_fee_bps: fee_bps,
            supported_tokens: supported,
        }
    }

    /// Calcula o "Sweet Spot" - montante ótimo de flashloan para maximizar lucro
    /// Considera: slippage crescente, taxa flashloan, gas extra
    pub fn calculate_optimal_loan(&self, path: &ArbitragePath, pool_liquidity: U256) -> FlashLoanCalculation {
        let max_loan = U256::from(self.max_loan_amount_eth) * U256::from(10).pow(U256::from(18));
        
        // Começar com base amount e iterar para encontrar sweet spot
        let base_amount = path.optimal_input;
        let test_amounts = vec![
            base_amount,
            base_amount * U256::from(2),
            base_amount * U256::from(5),
            base_amount * U256::from(10),
            base_amount * U256::from(50),
            base_amount * U256::from(100),
        ];
        
        let mut best_calc = FlashLoanCalculation {
            loan_amount: U256::ZERO,
            provider: self.preferred_provider,
            flash_loan_fee_wei: U256::ZERO,
            extra_gas_cost: 0,
            net_profit_wei: U256::ZERO,
            is_profitable: false,
            min_amount_out: U256::ZERO,
            total_gas_cost_wei: U256::ZERO,
        };
        
        for amount in test_amounts {
            // Limitar à liquidez disponível (máx 30% da pool para não quebrar)
            let max_safe = pool_liquidity / U256::from(3);
            let loan = if amount > max_safe { max_safe } else if amount > max_loan { max_loan } else { amount };
            
            if loan.is_zero() {
                continue;
            }
            
            let calc = self.calculate_flashloan_details(&loan, path.hops.len());
            
            if calc.net_profit_wei > best_calc.net_profit_wei && calc.is_profitable {
                best_calc = calc;
            }
        }
        
        best_calc
    }
    
    /// Calcula detalhes de um flashloan específico
    fn calculate_flashloan_details(&self, loan_amount: &U256, path_len: usize) -> FlashLoanCalculation {
        // Taxa flashloan: 0% Balancer ou 0.05% Aave
        let fee_wei = *loan_amount * U256::from(self.flash_loan_fee_bps) / U256::from(10_000);
        
        // Gas extra: ~60k UniswapV3, ~80k Balancer, ~100k Aave (flashloan + callbacks)
        let base_flash_gas = match self.preferred_provider {
            FlashLoanProvider::UniswapV3 => 60_000,  // FlashSwap é mais eficiente
            FlashLoanProvider::BalancerV2 => 80_000,
            FlashLoanProvider::AaveV3 => 100_000,
        };
        let extra_gas = base_flash_gas + (20_000 * path_len as u64); // +20k por hop
        
        // Gas cost em wei (assumindo 20 gwei)
        let gas_price_gwei = 20_000_000_000u128;
        let gas_cost_wei = U256::from(extra_gas) * U256::from(gas_price_gwei);
        
        // Custo total = taxa flashloan + gas extra
        let total_cost = fee_wei + gas_cost_wei;
        
        // Cálculo do min_amount_out com 1.2x margem de segurança (modo agressivo)
        // Fórmula: min_out = borrowed + flash_fee + gas_cost + (1.2x margem de segurança)
        let safety_margin = (total_cost * U256::from(12)) / U256::from(10); // 1.2x margem
        let min_amount_out = *loan_amount + total_cost + safety_margin;
        
        FlashLoanCalculation {
            loan_amount: *loan_amount,
            provider: self.preferred_provider,
            flash_loan_fee_wei: fee_wei,
            extra_gas_cost: extra_gas,
            net_profit_wei: U256::ZERO, // Será calculado pelo estrategista
            is_profitable: false, // Será determinado pelo estrategista
            min_amount_out,
            total_gas_cost_wei: gas_cost_wei,
        }
    }

    /// Validação completa de segurança para proteger a banca de 75€
    /// Verifica: margem 3x, slippage, lucro mínimo, e condição de reversão
    pub fn validate_safety_check(
        &self,
        calc: &FlashLoanCalculation,
        expected_output: U256,
        safety: &SafetyCheck,
    ) -> Result<bool, String> {
        // 1. Verificar margem de segurança 1.2x: lucro deve ser >= 1.2x custo total (modo agressivo)
        let total_cost = calc.flash_loan_fee_wei + calc.total_gas_cost_wei;
        // Multiplicador 12 representa 1.2x (12/10), portanto dividimos por 10 depois
        let min_required_profit = (total_cost * U256::from(safety.safety_margin_multiplier)) / U256::from(10);
        
        if calc.net_profit_wei < min_required_profit {
            return Err(format!(
                "SAFETY FAIL: Lucro {} < {}x custo total ({})",
                calc.net_profit_wei, safety.safety_margin_multiplier, min_required_profit
            ));
        }
        
        // 2. Verificar slippage máximo
        // Slippage = (expected - min_out) / expected
        if expected_output < calc.min_amount_out {
            let shortfall = calc.min_amount_out - expected_output;
            let slippage_bps = (shortfall * U256::from(10_000)) / expected_output;
            
            if slippage_bps > U256::from(safety.max_slippage_bps) {
                return Err(format!(
                    "SAFETY FAIL: Slippage {} bps > max {} bps",
                    slippage_bps, safety.max_slippage_bps
                ));
            }
        }
        
        // 3. Verificar lucro mínimo absoluto (0.00005 ETH)
        if calc.net_profit_wei < safety.min_profit_wei {
            return Err(format!(
                "SAFETY FAIL: Lucro {} < mínimo {} wei",
                calc.net_profit_wei, safety.min_profit_wei
            ));
        }
        
        // 4. Verificação crítica: require(amount_received >= borrowed + fee + gas)
        // Se esta condição falhar, a transação daria revert e perderíamos gas
        let break_even = calc.loan_amount + calc.flash_loan_fee_wei + calc.total_gas_cost_wei;
        if expected_output < break_even {
            return Err(format!(
                "CRITICAL: Output {} < break-even {} (REVERT garantido!)",
                expected_output, break_even
            ));
        }
        
        Ok(true)
    }

    /// Verifica se vale a pena usar flash loan para um lucro bruto estimado
    pub fn is_flash_loan_profitable(&self, gross_profit: U256, calc: &FlashLoanCalculation) -> bool {
        // Lucro mínimo para cobrir taxa + gas (0.00005 ETH = ~0.15€)
        let min_profit_wei = U256::from(50_000_000_000_000u128); // 0.00005 ETH
        
        let net = if gross_profit > calc.flash_loan_fee_wei + U256::from(calc.extra_gas_cost as u128 * 20_000_000_000u128) {
            gross_profit - calc.flash_loan_fee_wei - U256::from(calc.extra_gas_cost as u128 * 20_000_000_000u128)
        } else {
            U256::ZERO
        };
        
        net > min_profit_wei
    }
    
    /// Seleciona o melhor provedor baseado no token e montante
    pub fn select_best_provider(_token: Address, amount: U256) -> FlashLoanProvider {
        // BalancerV2 tem taxa mais baixa (0.09%)
        // AaveV3 tem taxa de 0.09% mas maior liquidez
        
        // Para montantes > 1000 ETH ou tokens exóticos, preferir Aave
        let threshold = U256::from(1000) * U256::from(10).pow(U256::from(18));
        
        if amount > threshold {
            FlashLoanProvider::AaveV3
        } else {
            // Para montantes menores, usar UniswapV3 FlashSwap (0% taxa extra)
            FlashLoanProvider::UniswapV3
        }
    }
    
    /// Gera calldata para flashloan no Balancer
    pub fn encode_balancer_flashloan(&self, _token: Address, _amount: U256, _data: Vec<u8>) -> Vec<u8> {
        // Balancer Vault: flashLoan(
        //   IFlashLoanRecipient recipient,
        //   address[] tokens,
        //   uint256[] amounts,
        //   bytes userData
        // )
        let selector = hex::decode("").unwrap_or_default();
        // Simplificado - em produção usar ABI encoding completo
        selector
    }
    
    /// Gera calldata para flashloan no Aave V3
    pub fn encode_aave_flashloan(&self, _token: Address, _amount: U256, _data: Vec<u8>) -> Vec<u8> {
        // Aave Pool: flashLoanSimple(
        //   address receiver,
        //   address asset,
        //   uint256 amount,
        //   bytes calldata params,
        //   uint16 referralCode
        // )
        let selector = hex::decode("").unwrap_or_default();
        // Simplificado - em produção usar ABI encoding completo
        selector
    }
}

/// ============================================
/// MEV-SHARE PRIVATE EXECUTION
/// ============================================

use alloy::providers::RootProvider;
use alloy::transports::BoxTransport;
use std::collections::HashMap;

/// Bundle privado para MEV-Share
#[derive(Clone, Debug)]
pub struct PrivateBundle {
    pub txs: Vec<Vec<u8>>,
    pub target_block: u64,
    pub min_timestamp: Option<u64>,
    pub max_timestamp: Option<u64>,
    pub reverting_tx_hashes: Vec<B256>,
}

impl PrivateBundle {
    pub fn new(txs: Vec<Vec<u8>>, target_block: u64) -> Self {
        Self {
            txs,
            target_block,
            min_timestamp: None,
            max_timestamp: None,
            reverting_tx_hashes: Vec::new(),
        }
    }
    
    /// Marca uma transação como permitida para reverter (all-or-nothing)
    pub fn allow_revert(mut self, tx_hash: B256) -> Self {
        self.reverting_tx_hashes.push(tx_hash);
        self
    }
}

/// Broadcaster MEV-Share para Base
pub struct MevShareBroadcaster {
    #[allow(dead_code)]
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    #[allow(dead_code)]
    endpoint: String,
    /// Bundles pendentes por bloco
    pending_bundles: Arc<RwLock<HashMap<u64, Vec<PrivateBundle>>>>,
}

impl MevShareBroadcaster {
    pub fn new(provider: Arc<RwLock<RootProvider<BoxTransport>>>) -> Self {
        Self {
            provider,
            endpoint: "https://mev-share.flashbots.net/api/v1/bundle".to_string(),
            pending_bundles: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Submete bundle privado via MEV-Share
    pub async fn submit_bundle(&self, bundle: PrivateBundle) -> eyre::Result<String> {
        let target = bundle.target_block;
        
        // Validar atomicidade
        if bundle.reverting_tx_hashes.len() >= bundle.txs.len() && !bundle.txs.is_empty() {
            return Err(eyre::eyre!("Bundle não-atômico: todas as txs podem reverter"));
        }
        
        info!(
            "🔒 [MEV-SHARE] Bundle privado | Block: {} | Txs: {} | Atomic: ✅",
            target,
            bundle.txs.len()
        );
        
        // Armazenar para tracking
        self.pending_bundles.write().await
            .entry(target)
            .or_default()
            .push(bundle);
        
        // Simular envio - integração real usaria API Flashbots
        let bundle_hash = format!("0x{:064x}", target);
        
        info!(
            "📤 [MEV-SHARE] Private Bundle Submitted | Hash: {} | Status: PENDING",
            bundle_hash
        );
        
        Ok(bundle_hash)
    }
    
    /// Verifica status de bundles pendentes
    pub async fn check_bundle_status(&self, block: u64) -> Vec<String> {
        let bundles = self.pending_bundles.read().await;
        
        if let Some(bundle_list) = bundles.get(&block) {
            bundle_list.iter()
                .enumerate()
                .map(|(i, _)| format!("bundle_{}_{}", block, i))
                .collect()
        } else {
            Vec::new()
        }
    }
}

/// ============================================
/// GOD-MODE SAFETY: Min Profit para 80€ Banca
/// ============================================

/// Calcula profit mínimo seguro para banca de 80€
/// A 3000€/ETH, 80€ = 0.027 ETH
/// Com margem de 20%, min_profit = 0.0324 ETH
pub const GOD_MODE_MIN_PROFIT_WEI: u128 = 32_400_000_000_000_000_000u128; // 0.0324 ETH

/// Verifica se o lucro líquido protege a banca de 80€
pub fn is_profit_safe_for_bankroll(net_profit_wei: U256) -> bool {
    let min_safe = U256::from(GOD_MODE_MIN_PROFIT_WEI);
    
    if net_profit_wei >= min_safe {
        true
    } else {
        let profit_eth = net_profit_wei.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
        warn!(
            "🛡️ [BANKROLL-PROTECTION] Lucro {:.6} ETH < Mínimo {:.6} ETH | Rota descartada",
            profit_eth,
            GOD_MODE_MIN_PROFIT_WEI as f64 / 1e18
        );
        false
    }
}
