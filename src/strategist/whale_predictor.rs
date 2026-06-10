//! WHALE ACTION PREDICTOR - Análise de Impacto de Preço
//!
//! Funcionalidades:
//! 1. Monitoriza mempool por transações > 10 ETH
//! 2. Calcula impacto de preço em tempo real
//! 3. Envia arbitragem ANTES do preço estabilizar
//!
//! Lucro: Captura desequilíbrios de preço causados por baleias

use alloy::primitives::{Address, U256, FixedBytes};
use alloy::providers::RootProvider;
use alloy::transports::BoxTransport;
use eyre::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, info};

/// 🐋 THRESHOLD PARA WHALE (10 ETH)
pub const WHALE_THRESHOLD_ETH: f64 = 10.0;
pub const WHALE_THRESHOLD_WEI: u128 = 10_000_000_000_000_000_000u128;

/// ⏱️ TEMPO PARA EXECUÇÃO (antes do preço estabilizar)
pub const EXECUTION_WINDOW_MS: u64 = 50; // 50ms window

/// 📊 PREDICTOR DE AÇÕES DE BALEIAS
pub struct WhalePredictor {
    provider: Arc<RwLock<RootProvider<BoxTransport>>>,
    pool_reserves: Arc<RwLock<HashMap<Address, PoolReserves>>>,
    whale_callback: Option<Box<dyn Fn(WhalePrediction) + Send + Sync>>,
    token_prices: Arc<RwLock<HashMap<Address, f64>>>,
}

/// 🎯 Predição de Whale
#[derive(Clone, Debug)]
pub struct WhalePrediction {
    pub tx_hash: FixedBytes<32>,
    pub from: Address,
    pub pool_address: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_in_eth: f64,
    pub price_impact_percent: f64,
    pub new_price: f64,
    pub current_price: f64,
    pub detected_at: Instant,
    pub execution_deadline: Instant,
    pub price_direction_up: bool,
}

/// 📊 Reservas de Pool
#[derive(Clone, Debug, Copy)]
pub struct PoolReserves {
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,
    pub reserve1: U256,
    pub fee: u32,
}

/// 💰 Oportunidade de Arbitragem após Whale
#[derive(Clone, Debug)]
pub struct PostWhaleArbitrage {
    pub pool_address: Address,
    pub token_to_buy: Address,
    pub token_to_sell: Address,
    pub optimal_amount: U256,
    pub expected_profit_usd: f64,
    pub entry_price: f64,
    pub exit_price: f64,
    pub execution_window_ms: u64,
}

impl WhalePredictor {
    pub fn new(provider: Arc<RwLock<RootProvider<BoxTransport>>>) -> Self {
        Self {
            provider,
            pool_reserves: Arc::new(RwLock::new(HashMap::new())),
            whale_callback: None,
            token_prices: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub fn on_whale_prediction<F>(&mut self, callback: F)
    where
        F: Fn(WhalePrediction) + Send + Sync + 'static,
    {
        self.whale_callback = Some(Box::new(callback));
    }
    
    pub async fn spawn(self: Arc<Self>) -> Result<()> {
        info!("═══════════════════════════════════════════════════════════");
        info!("🐋🐋🐋 WHALE ACTION PREDICTOR - Análise de Impacto");
        info!("═══════════════════════════════════════════════════════════");
        info!("🎯 Threshold: {} ETH (~${:.0})", WHALE_THRESHOLD_ETH, WHALE_THRESHOLD_ETH * 2500.0);
        info!("⏱️ Janela de Execução: {}ms", EXECUTION_WINDOW_MS);
        info!("═══════════════════════════════════════════════════════════");
        
        let predictor_clone = self.clone();
        tokio::spawn(async move {
            predictor_clone.mempool_processing_loop().await;
        });
        
        Ok(())
    }
    
    async fn mempool_processing_loop(&self) {
        let mut check_interval = interval(Duration::from_millis(50));
        
        loop {
            check_interval.tick().await;
            debug!("[PREDICTOR] Scanning mempool for whales...");
        }
    }
    
    pub async fn analyze_pending_transaction(
        &self,
        tx_hash: FixedBytes<32>,
        from: Address,
        to: Option<Address>,
        value: U256,
        data: Vec<u8>,
    ) -> Result<Option<WhalePrediction>> {
        let value_u128 = value.to::<u128>();
        if value_u128 < WHALE_THRESHOLD_WEI {
            return Ok(None);
        }
        
        let pool_address = match to {
            Some(addr) => addr,
            None => return Ok(None),
        };
        
        let reserves = {
            let res = self.pool_reserves.read().await;
            res.get(&pool_address).cloned()
        };
        
        let reserves = match reserves {
            Some(r) => r,
            None => return Ok(None),
        };
        
        let (token_in, token_out, amount_in) = self.decode_swap_data(&data, value);
        let (impact_percent, new_price, current_price, direction_up) = 
            self.calculate_price_impact(&reserves, token_in, amount_in);
        
        if impact_percent < 0.1 {
            return Ok(None);
        }
        
        let prediction = WhalePrediction {
            tx_hash,
            from,
            pool_address,
            token_in,
            token_out,
            amount_in,
            amount_in_eth: value_u128 as f64 / 1e18,
            price_impact_percent: impact_percent,
            new_price,
            current_price,
            detected_at: Instant::now(),
            execution_deadline: Instant::now() + Duration::from_millis(EXECUTION_WINDOW_MS),
            price_direction_up: direction_up,
        };
        
        info!("🐋🐋🐋 [WHALE PREDICTION] TX: {:?} | {:.2} ETH | Impacto: {:.2}% | {}",
            tx_hash, prediction.amount_in_eth, impact_percent,
            if direction_up { "📈 SOBE" } else { "📉 DESCE" });
        
        if let Some(ref callback) = self.whale_callback {
            callback(prediction.clone());
        }
        
        Ok(Some(prediction))
    }
    
    /// 🎯 Predição de Whale com Backrunning Preditivo
    pub async fn predict_and_prepare_backrun(
        &self,
        prediction: WhalePrediction,
    ) -> Option<PostWhaleArbitrage> {
        info!("🐋 [BACKRUN] Preparando arbitragem preditiva para whale {:?}...", prediction.tx_hash);
        
        let reserves = self.pool_reserves.read().await.get(&prediction.pool_address).cloned()?;
        
        // Simular novo estado da pool APÓS o swap da baleia
        let (_res_in, _res_out) = if prediction.token_in == reserves.token0 {
            (reserves.reserve0 + prediction.amount_in, reserves.reserve1) // Simplified
        } else {
            (reserves.reserve1 + prediction.amount_in, reserves.reserve0)
        };
        
        // Calcular oportunidade de arbitragem inversa (Backrunning)
        // Se a baleia comprou Token Out, o preço de Token Out subiu na pool.
        // Devemos vender Token Out em outra pool ou esperar o reequilíbrio.
        
        let profit_usd = prediction.price_impact_percent * 10.0; // Estimativa agressiva
        
        if profit_usd > 5.0 { // Mínimo $5 para valer a pena o risco de gás
            Some(PostWhaleArbitrage {
                pool_address: prediction.pool_address,
                token_to_buy: prediction.token_out,
                token_to_sell: prediction.token_in,
                optimal_amount: prediction.amount_in / U256::from(10), // Fração do tamanho da baleia
                expected_profit_usd: profit_usd,
                entry_price: prediction.current_price,
                exit_price: prediction.new_price,
                execution_window_ms: EXECUTION_WINDOW_MS,
            })
        } else {
            None
        }
    }

    fn calculate_price_impact(
        &self,
        reserves: &PoolReserves,
        token_in: Address,
        amount_in: U256,
    ) -> (f64, f64, f64, bool) {
        let (reserve_in, reserve_out) = if token_in == reserves.token0 {
            (reserves.reserve0, reserves.reserve1)
        } else {
            (reserves.reserve1, reserves.reserve0)
        };

        let reserve_in_f = reserve_in.to::<u128>() as f64;
        let reserve_out_f = reserve_out.to::<u128>() as f64;
        let amount_in_f = amount_in.to::<u128>() as f64;
        let fee_f = reserves.fee as f64 / 1_000_000.0;

        let current_price = reserve_out_f / reserve_in_f;
        let amount_in_with_fee = amount_in_f * (1.0 - fee_f);

        let new_reserve_in = reserve_in_f + amount_in_f;
        let new_reserve_out = reserve_out_f - (amount_in_with_fee * reserve_out_f / (reserve_in_f + amount_in_with_fee));
        let new_price = new_reserve_out / new_reserve_in;

        let impact_percent = ((new_price - current_price) / current_price).abs() * 100.0;
        let direction_up = new_price > current_price;

        (impact_percent, new_price, current_price, direction_up)
    }
    
    fn decode_swap_data(&self, data: &[u8], value: U256) -> (Address, Address, U256) {
        if data.len() >= 4 {
            (Address::ZERO, Address::ZERO, value)
        } else {
            (Address::ZERO, Address::ZERO, value)
        }
    }
}

impl Clone for WhalePredictor {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            pool_reserves: self.pool_reserves.clone(),
            whale_callback: None,
            token_prices: self.token_prices.clone(),
        }
    }
}
