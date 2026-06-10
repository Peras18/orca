//! SAFETY & RISK MANAGEMENT MODULE
//! Proteção de capital, kill-switches, e controlo de execução

pub mod mev_share_executor;
pub mod dynamic_profit;
pub mod kill_switch;

pub use mev_share_executor::{MevShareExecutor, BundleStatus, MevBundle};
pub use dynamic_profit::ProfitAdaptiveEngine;
pub use kill_switch::{KillSwitch, CapitalGuard, RiskLevel};

use tracing::{info, error, warn};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 🛡️ Centro de Segurança do Bot
#[derive(Clone, Debug)]
pub struct SafetyEngine {
    /// Executor MEV-Share
    pub mev_executor: MevShareExecutor,
    /// Threshold de profit dinâmico
    pub profit_adaptive: ProfitAdaptiveEngine,
    /// Kill-Switch de capital
    pub kill_switch: KillSwitch,
    /// Capital inicial (€)
    pub initial_capital: f64,
    /// Capital atual (€)
    current_capital: Arc<RwLock<f64>>,
    /// Status geral
    pub system_status: Arc<RwLock<SystemStatus>>,
}

/// 🚦 Status do Sistema
#[derive(Clone, Debug, PartialEq)]
pub enum SystemStatus {
    /// Operando normalmente
    Active,
    /// Em pausa (sem trades recentes)
    Idle,
    /// Kill-switch ativado
    Halted,
    /// Aguardando autorização
    AwaitingAuth,
}

impl SafetyEngine {
    /// 🚀 Inicializa sistema de segurança
    pub fn new(initial_capital_eur: f64) -> Self {
        info!("═══════════════════════════════════════════════════════════");
        info!("🛡️ SAFETY ENGINE - Proteção de Capital Ativada");
        info!("💰 Capital Inicial: {}€", initial_capital_eur);
        info!("📉 Kill-Switch: -50% = {}€", initial_capital_eur * 0.5);
        info!("🎯 Profit Dinâmico: 5€ → 2€ (adaptativo)");
        info!("⚡ MEV-Share: Cancela se não for topo com lucro");
        info!("═══════════════════════════════════════════════════════════");
        
        Self {
            mev_executor: MevShareExecutor::new(),
            profit_adaptive: ProfitAdaptiveEngine::new(initial_capital_eur),
            kill_switch: KillSwitch::new(initial_capital_eur),
            initial_capital: initial_capital_eur,
            current_capital: Arc::new(RwLock::new(initial_capital_eur)),
            system_status: Arc::new(RwLock::new(SystemStatus::Active)),
        }
    }
    
    /// 💰 Atualiza capital após trade
    pub async fn update_capital(&self, change_eur: f64) {
        let mut capital = self.current_capital.write().await;
        *capital += change_eur;
        
        let new_capital = *capital;
        drop(capital);
        
        // Verificar kill-switch
        if self.kill_switch.check_threshold(new_capital).await {
            self.trigger_kill_switch(new_capital).await;
        }
        
        info!(
            "[SAFETY] 💰 Capital atualizado: {}€ (variação: {:+.2}€)",
            new_capital, change_eur
        );
    }
    
    /// 🚨 Ativa kill-switch
    async fn trigger_kill_switch(&self, current_capital: f64) {
        let mut status = self.system_status.write().await;
        *status = SystemStatus::Halted;
        
        error!("═══════════════════════════════════════════════════════════");
        error!("🚨 KILL-SWITCH ATIVADO! 🚨");
        error!("💸 Capital caiu de {}€ para {}€", self.initial_capital, current_capital);
        error!("📉 Perda de {:.1}% - Limite de segurança atingido", 
            (1.0 - current_capital / self.initial_capital) * 100.0);
        error!("⛔ TODAS AS OPERAÇÕES PARADAS");
        error!("🔑 Aguardando autorização manual para continuar...");
        error!("═══════════════════════════════════════════════════════════");
        
        // Aqui poderia enviar notificação (email, SMS, etc.)
    }
    
    /// 🔓 Requer autorização para continuar após kill-switch
    pub async fn authorize_continue(&self, auth_code: &str) -> bool {
        if auth_code != "APEX_RESUME_2024" {
            warn!("[SAFETY] ❌ Código de autorização inválido");
            return false;
        }
        
        let mut status = self.system_status.write().await;
        if *status == SystemStatus::Halted || *status == SystemStatus::AwaitingAuth {
            *status = SystemStatus::Active;
            
            // Reset kill-switch com novo capital de referência
            let current = *self.current_capital.read().await;
            self.kill_switch.reset(current).await;
            
            info!("[SAFETY] 🔓 Sistema retomado com autorização");
            info!("[SAFETY] 🔄 Novo reference capital: {}€", current);
            true
        } else {
            false
        }
    }
    
    /// 🎯 Verifica se pode executar trade
    pub async fn can_execute(&self, expected_profit_eur: f64) -> bool {
        let status = self.system_status.read().await;
        
        if *status == SystemStatus::Halted {
            return false;
        }
        
        // Verificar threshold dinâmico
        let threshold = self.profit_adaptive.current_threshold().await;
        
        if expected_profit_eur < threshold {
            trace!(
                "[SAFETY] ⛔ Profit {}€ abaixo do threshold {}€",
                expected_profit_eur, threshold
            );
            return false;
        }
        
        true
    }
    
    /// 📊 Estatísticas de segurança
    pub async fn stats(&self) -> String {
        let capital = *self.current_capital.read().await;
        let status = self.system_status.read().await.clone();
        let threshold = self.profit_adaptive.current_threshold().await;
        
        format!(
            "🛡️ Safety | Capital: {}€/{}€ | Status: {:?} | Threshold: {}€ | Kill: {}€",
            capital,
            self.initial_capital,
            status,
            threshold,
            self.kill_switch.kill_threshold().await
        )
    }
}

use tracing::trace;
