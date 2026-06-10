//! 📦 Transaction Bundle Builder
use alloy::primitives::{Address, U256, Bytes};
use crate::types::ArbitragePath;
use crate::executor::FlashLoanProvider;

#[derive(Clone, Debug)]
pub struct TxBundle {
    pub payloads: Vec<BundlePayload>,
    pub total_gas_limit: u64,
    pub total_priority_fee: u128,
    pub flash_provider: FlashLoanProvider,
    pub loan_amount: U256,
    pub min_profit: U256,
    pub deadline_block: u64,
}

#[derive(Clone, Debug)]
pub struct BundlePayload {
    pub target: Address,
    pub calldata: Bytes,
    pub value: U256,
    pub gas_limit: u64,
    pub priority_fee: u128,
}

impl TxBundle {
    pub fn new(flash_provider: FlashLoanProvider, loan: U256, min_profit: U256, deadline: u64) -> Self {
        Self {
            payloads: Vec::new(),
            total_gas_limit: 0,
            total_priority_fee: 0,
            flash_provider,
            loan_amount: loan,
            min_profit,
            deadline_block: deadline,
        }
    }

    pub fn add_payload(&mut self, payload: BundlePayload) {
        self.total_gas_limit += payload.gas_limit;
        self.total_priority_fee = self.total_priority_fee.max(payload.priority_fee);
        self.payloads.push(payload);
    }

    pub fn encode_arb_path(&self, path: &ArbitragePath, executor: Address) -> Bytes {
        // Encode: [executor:20][loanToken:20][loanAmount:32][blockDeadline:4][minProfit:4][hopCount:1][hops...]
        // CRÍTICO: executor + loanToken + loanAmount no início para o contrato validar e saber quanto pedir
        let mut encoded = Vec::with_capacity(81 + path.hops.len() * 25);

        // 1. Executor address (20 bytes) — para o contrato verificar autorização on-chain
        encoded.extend_from_slice(executor.as_slice());

        // 2. Loan token address (20 bytes) — WETH na Base
        let weth = alloy::primitives::address!("0x4200000000000000000000000000000000000006");
        encoded.extend_from_slice(weth.as_slice());

        // 3. Loan amount (32 bytes) — valor completo em wei
        // 🚨 CORREÇÃO: Garantir que loan_amount > 0
        assert!(!self.loan_amount.is_zero(), "loan_amount cannot be zero");
        let loan_bytes = self.loan_amount.to_be_bytes_vec();
        assert_eq!(loan_bytes.len(), 32, "loan_amount must be 32 bytes");
        encoded.extend_from_slice(&loan_bytes);

        // 4. Block deadline (4 bytes)
        encoded.extend_from_slice(&(self.deadline_block as u32).to_be_bytes());

        // 5. Min profit compactado (4 bytes): wei / 1e9
        let min_profit_compact = (self.min_profit / U256::from(1_000_000_000)).to::<u32>();
        encoded.extend_from_slice(&min_profit_compact.to_be_bytes());

        // 6. Hop count (1 byte)
        encoded.push(path.hops.len() as u8);

        for hop in &path.hops {
            encoded.extend_from_slice(hop.pool.as_slice());
            let token_suffix = &hop.token_in.as_slice()[16..20];
            encoded.extend_from_slice(token_suffix);
            let dex_flag = match hop.dex_type {
                crate::contracts::DexType::Aerodrome | crate::contracts::DexType::AerodromeStable => 0x80,
                _ => 0x00,
            };
            let fee_index = match hop.fee {
                500 => 0,
                3000 => 1,
                10000 => 2,
                _ => 1,
            };
            encoded.push(dex_flag | fee_index);
        }

        Bytes::from(encoded)
    }
}

/// Builder for atomic execution
pub struct AtomicBundleBuilder;

impl AtomicBundleBuilder {
    pub fn build(
        path: &ArbitragePath,
        flash_loan: U256,
        executor: Address,
        deadline: u64,
    ) -> TxBundle {
        let mut bundle = TxBundle::new(FlashLoanProvider::BalancerV2, flash_loan, U256::from(50000), deadline);
        
        let payload = BundlePayload {
            target: executor,
            calldata: bundle.encode_arb_path(path, executor),
            value: U256::ZERO,
            gas_limit: 500_000,
            priority_fee: 1_000_000_000,
        };
        
        bundle.add_payload(payload);
        bundle
    }
}
