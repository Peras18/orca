//! PRIORITY GAS AUCTION (PGA) - Sistema de Lance de Gas
//!
//! Funcionalidade: Se lucro for de 500€, autoriza gastar até 200€ em gas
//! para garantir inclusão no bloco.
//!
//! Target: Vencer qualquer competição de MEV na Base Mainnet

use alloy::primitives::U256;
use std::sync::Arc;
use std::time::{Instant, Duration};
use tokio::sync::RwLock;
use tracing::{info, warn, debug, trace};

/// ⛽ CONFIGURAÇÃO PGA (em EUR)
pub const PGA_PROFIT_THRESHOLD_EUR: f64 = 100.0;      // Lucro mínimo para PGA (ajustado)
pub const PGA_MAX_GAS_EUR: f64 = 80.0;                // Máximo gastar em gas (User target: 80€)
pub const PGA_NORMAL_GAS_EUR: f64 = 2.0;              // Gas normal (2€)
pub const PGA_AGGRESSIVE_GAS_EUR: f64 = 20.0;         // Gas agressivo (20€)
pub const USER_BANKROLL_EUR: f64 = 80.0;              // Banca total do utilizador
pub const MAX_SINGLE_TX_LOSS_PCT: f64 = 0.20;         // Máximo 20% da banca por falha (16€)

/// 💰 STRATEGY TIERS
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GasStrategy {
    Conservative,  // Lucro < 50€ → Gas 0.5€
    Normal,        // 50€ < Lucro < 200€ → Gas 2€
    Aggressive,    // 200€ < Lucro < 500€ → Gas 20€
    Nuclear,       // Lucro > 500€ → Gas 80€ (máximo user-defined)
}

/// 🎯 PGA CONTROLLER
pub struct GasAuctionController {
    /// Preço atual do gas (gwei)
    current_gas_price: Arc<RwLock<u64>>,
    /// Preço do ETH (EUR)
    eth_price_eur: Arc<RwLock<f64>>,
    /// Contador de lances ganhos
    winning_bids: Arc<RwLock<u64>>,
    /// Total gasto em gas (EUR)
    total_gas_spent_eur: Arc<RwLock<f64>>,
    /// Lucro total (EUR)
    total_profit_eur: Arc<RwLock<f64>>,
}

/// 📊 Bid Package para envio
#[derive(Clone, Debug)]
pub struct GasBid {
    /// Hash da transação
    pub tx_hash: String,
    /// Lucro esperado (EUR)
    pub expected_profit_eur: f64,
    /// Gas oferecido (wei)
    pub gas_price_wei: U256,
    /// Gas limit estimado
    pub gas_limit: u64,
    /// Custo total de gas (EUR)
    pub gas_cost_eur: f64,
    /// Estratégia usada
    pub strategy: GasStrategy,
    /// Timestamp do bid
    pub timestamp: Instant,
    /// Deadline para inclusão
    pub deadline: Instant,
}

impl GasAuctionController {
    pub fn new() -> Self {
        Self {
            current_gas_price: Arc::new(RwLock::new(20_000_000_000)), // 20 gwei default
            eth_price_eur: Arc::new(RwLock::new(2300.0)), // ETH @ 2300 EUR
            winning_bids: Arc::new(RwLock::new(0)),
            total_gas_spent_eur: Arc::new(RwLock::new(0.0)),
            total_profit_eur: Arc::new(RwLock::new(0.0)),
        }
    }
    
    /// 🚀 Inicializa o PGA
    pub async fn spawn(self: Arc<Self>) {
        info!("═══════════════════════════════════════════════════════════");
        info!("⛽⛽⛽ PRIORITY GAS AUCTION - Sistema de Lance Vitória");
        info!("═══════════════════════════════════════════════════════════");
        info!("🎯 Threshold PGA: {}€ lucro", PGA_PROFIT_THRESHOLD_EUR);
        info!("🔥 Max Gas: {}€ | Agressive: {}€ | Normal: {}€", 
            PGA_MAX_GAS_EUR, PGA_AGGRESSIVE_GAS_EUR, PGA_NORMAL_GAS_EUR);
        info!("💡 Lógica: 'Melhor gastar 200€ em gas do que perder 500€ de lucro'");
        info!("═══════════════════════════════════════════════════════════");
        
        // Spawn price updater
        let controller = self.clone();
        tokio::spawn(async move {
            controller.gas_price_updater().await;
        });
    }
    
    /// 💰 Calcula estratégia de gas com AUTO-SCALING baseado no lucro acumulado
    pub async fn calculate_autoscaled_gas_bid(
        &self,
        tx_hash: String,
        expected_profit_eur: f64,
        gas_limit: u64,
    ) -> Option<GasBid> {
        let total_profit = *self.total_profit_eur.read().await;
        let eth_price = *self.eth_price_eur.read().await;
        
        // Multiplicador de agressividade baseado no lucro acumulado
        // Se já lucramos 1000€, podemos ser 2x mais agressivos
        let profit_multiplier = 1.0 + (total_profit / 1000.0).min(4.0);
        
        let strategy = self.select_strategy(expected_profit_eur);
        
        let mut target_gas_eur = match strategy {
            GasStrategy::Conservative => 0.5,
            GasStrategy::Normal => 2.0,
            GasStrategy::Aggressive => 20.0,
            GasStrategy::Nuclear => 80.0,
        };
        
        // Aplicar auto-scaling
        target_gas_eur *= profit_multiplier;
        
        // Cap absoluto de 200€ por transação para evitar drenar a banca
        target_gas_eur = target_gas_eur.min(200.0);
        
        // 🛡️ HARD STOP: Nunca gastar mais de 20% da banca (16€) se o risco for de perda total
        let hard_stop_limit = USER_BANKROLL_EUR * MAX_SINGLE_TX_LOSS_PCT;
        if target_gas_eur > hard_stop_limit {
            debug!("[PGA-SAFE] 🛡️ Capando bid de {:.2}€ para {:.2}€ (Hard Stop 20% da banca)", 
                target_gas_eur, hard_stop_limit);
            target_gas_eur = hard_stop_limit;
        }
        
        let gas_cost_eth = target_gas_eur / eth_price;
        let gas_price_wei = (gas_cost_eth * 1e18 / gas_limit as f64) as u64;
        
        info!("[PGA-SCALING] 📈 Lucro Acumulado: {:.2}€ | Multiplicador: {:.1}x | Gas Bid: {:.2}€",
            total_profit, profit_multiplier, target_gas_eur);

        Some(GasBid {
            tx_hash,
            expected_profit_eur,
            gas_price_wei: U256::from(gas_price_wei),
            gas_limit,
            gas_cost_eur: target_gas_eur,
            strategy,
            timestamp: Instant::now(),
            deadline: Instant::now() + Duration::from_secs(12),
        })
    }
    
    /// 💰 Calcula estratégia de gas baseada no lucro
    pub async fn calculate_gas_bid(
        &self,
        tx_hash: String,
        expected_profit_eur: f64,
        gas_limit: u64,
    ) -> Option<GasBid> {
        let _gas_price = *self.current_gas_price.read().await;
        let eth_price = *self.eth_price_eur.read().await;
        
        // Determinar estratégia
        let strategy = self.select_strategy(expected_profit_eur);
        
        // Calcular gas em EUR baseado na estratégia
        let (target_gas_eur, reason) = match strategy {
            GasStrategy::Conservative => {
                (2.0, "Lucro < 100€, gas conservador".to_string())
            }
            GasStrategy::Normal => {
                (PGA_NORMAL_GAS_EUR, format!("Lucro {:.0}€, gas normal", expected_profit_eur))
            }
            GasStrategy::Aggressive => {
                (PGA_AGGRESSIVE_GAS_EUR, format!("🔥 Lucro {:.0}€ > 500€, gas agressivo", expected_profit_eur))
            }
            GasStrategy::Nuclear => {
                (PGA_MAX_GAS_EUR, format!("🔥🔥🔥 Lucro {:.0}€ > 1000€, gas NUCLEAR!", expected_profit_eur))
            }
        };
        
        // Converter EUR para wei
        // gas_cost_eth = target_gas_eur / eth_price_eur
        // gas_price_wei = gas_cost_eth * 1e18 / gas_limit
        let gas_cost_eth = target_gas_eur / eth_price;
        let gas_price_wei = (gas_cost_eth * 1e18 / gas_limit as f64) as u64;
        
        // Garantir mínimo de 1 gwei
        let gas_price_wei = gas_price_wei.max(1_000_000_000);
        
        // Calcular custo real
        let actual_gas_cost_eth = (gas_price_wei as f64 * gas_limit as f64) / 1e18;
        let actual_gas_cost_eur = actual_gas_cost_eth * eth_price;
        
        // Verificar se ainda é lucrativo
        let net_profit = expected_profit_eur - actual_gas_cost_eur;
        if net_profit < 0.0 {
            warn!("⛽ [PGA] Transação não lucrativa após gas: {}€ lucro - {}€ gas = {}€", 
                expected_profit_eur, actual_gas_cost_eur, net_profit);
            return None;
        }
        
        info!("⛽💰 [PGA BID] {} | Lucro: {:.0}€ | Gas: {:.1}€ ({} gwei) | Líquido: {:.0}€",
            tx_hash, expected_profit_eur, actual_gas_cost_eur, 
            gas_price_wei / 1_000_000_000, net_profit);
        
        if strategy == GasStrategy::Nuclear || strategy == GasStrategy::Aggressive {
            info!("🔥🔥🔥 [PGA WAR MODE] {} | {}", tx_hash, reason);
        }
        
        Some(GasBid {
            tx_hash,
            expected_profit_eur,
            gas_price_wei: U256::from(gas_price_wei),
            gas_limit,
            gas_cost_eur: actual_gas_cost_eur,
            strategy,
            timestamp: Instant::now(),
            deadline: Instant::now() + tokio::time::Duration::from_secs(12),
        })
    }
    
    /// 🎯 Seleciona estratégia baseada no lucro
    fn select_strategy(&self, profit_eur: f64) -> GasStrategy {
        if profit_eur >= 1000.0 {
            GasStrategy::Nuclear      // > 1000€ = Gas 200€
        } else if profit_eur >= PGA_PROFIT_THRESHOLD_EUR {
            GasStrategy::Aggressive  // > 500€ = Gas 50€
        } else if profit_eur >= 100.0 {
            GasStrategy::Normal      // > 100€ = Gas 5€
        } else {
            GasStrategy::Conservative // < 100€ = Gas 2€
        }
    }
    
    /// ✅ Registra bid vencedor
    pub async fn record_winning_bid(&self, bid: &GasBid) {
        let mut wins = self.winning_bids.write().await;
        *wins += 1;
        drop(wins);
        
        let mut gas_spent = self.total_gas_spent_eur.write().await;
        *gas_spent += bid.gas_cost_eur;
        drop(gas_spent);
        
        let mut profit = self.total_profit_eur.write().await;
        *profit += bid.expected_profit_eur - bid.gas_cost_eur;
        drop(profit);
        
        info!("🏆🏆🏆 [PGA WIN] Bid vencedor! Gas gasto: {:.1}€ | Lucro líquido: {:.0}€",
            bid.gas_cost_eur, bid.expected_profit_eur - bid.gas_cost_eur);
    }
    
    /// 📊 Retorna estatísticas do PGA
    pub async fn get_stats(&self) -> PGAStats {
        PGAStats {
            winning_bids: *self.winning_bids.read().await,
            total_gas_spent_eur: *self.total_gas_spent_eur.read().await,
            total_profit_eur: *self.total_profit_eur.read().await,
            current_gas_price_gwei: *self.current_gas_price.read().await / 1_000_000_000,
            eth_price_eur: *self.eth_price_eur.read().await,
        }
    }
    
    /// 🔄 Atualiza preço do ETH
    pub async fn update_eth_price(&self, price_eur: f64) {
        let mut eth_price = self.eth_price_eur.write().await;
        *eth_price = price_eur;
        info!("💶 [PGA] Preço ETH atualizado: {:.0}€", price_eur);
    }
    
    /// 💡 Simula resultado de um bid competitivo
    pub async fn simulate_competitive_bid(
        &self,
        our_profit: f64,
        competitor_gas_gwei: u64,
    ) -> BidSimulation {
        let our_bid = self.calculate_gas_bid(
            "simulation".to_string(),
            our_profit,
            200_000, // gas limit
        ).await;
        
        match our_bid {
            Some(bid) => {
                let competitor_cost = (competitor_gas_gwei as f64 * 200_000.0 / 1e18) 
                    * *self.eth_price_eur.read().await;
                
                let we_win = bid.gas_price_wei.to::<u64>() > competitor_gas_gwei;
                
                BidSimulation {
                    our_gas_eur: bid.gas_cost_eur,
                    competitor_gas_eur: competitor_cost,
                    we_win,
                    profit_if_win: our_profit - bid.gas_cost_eur,
                    profit_if_lose: 0.0,
                }
            }
            None => BidSimulation {
                our_gas_eur: 0.0,
                competitor_gas_eur: 0.0,
                we_win: false,
                profit_if_win: 0.0,
                profit_if_lose: 0.0,
            }
        }
    }

    /// 🏁 Encerra sessão
    pub async fn shutdown(&self) {
        info!("[PGA] Encerrando Gas Auction Controller...");
    }

    /// 🔄 Atualizador de preço de gás (Simulado ou via RPC)
    pub async fn gas_price_updater(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(12));
        loop {
            interval.tick().await;
            
            // Em produção: chamar eth_gasPrice
            // Simulação: flutuação entre 10 e 30 gwei
            let mut gas = self.current_gas_price.write().await;
            let fluctuation = (Instant::now().elapsed().as_secs() % 10) as i64 - 5;
            let new_gas = (*gas as i64 + fluctuation * 1_000_000_000).max(5_000_000_000) as u64;
            *gas = new_gas;
            
            trace!("[PGA-TICK] Preço do Gás: {} gwei", new_gas / 1_000_000_000);
        }
    }
}

/// 📊 Estatísticas do PGA
#[derive(Clone, Debug)]
pub struct PGAStats {
    pub winning_bids: u64,
    pub total_gas_spent_eur: f64,
    pub total_profit_eur: f64,
    pub current_gas_price_gwei: u64,
    pub eth_price_eur: f64,
}

/// 🎲 Simulação de Bid Competitivo
#[derive(Clone, Debug)]
pub struct BidSimulation {
    pub our_gas_eur: f64,
    pub competitor_gas_eur: f64,
    pub we_win: bool,
    pub profit_if_win: f64,
    pub profit_if_lose: f64,
}

impl Default for GasAuctionController {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for GasAuctionController {
    fn clone(&self) -> Self {
        Self {
            current_gas_price: self.current_gas_price.clone(),
            eth_price_eur: self.eth_price_eur.clone(),
            winning_bids: self.winning_bids.clone(),
            total_gas_spent_eur: self.total_gas_spent_eur.clone(),
            total_profit_eur: self.total_profit_eur.clone(),
        }
    }
}
