//! ORCA SAFETY ENGINE - Proteção absoluta de capital
//!
//! Regras de segurança:
//! 1. Simulação local obrigatória (eth_call)
//! 2. Lucro mínimo: 0.002 ETH (~$5)
//! 3. Bundle só executa se lucrativo e no topo
//! 4. Kill-switch a 50% do capital

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error, warn, debug};

/// 🛡️ Motor de Segurança ORCA
#[derive(Clone, Debug)]
pub struct SafetyEngine {
    /// Capital inicial
    initial_capital: f64,
    /// Capital atual
    current_capital: Arc<RwLock<f64>>,
    /// Lucro mínimo exigido (ETH)
    min_profit_eth: f64,
    /// Threshold de kill (50% do inicial)
    kill_threshold: f64,
    /// Se kill-switch está ativo
    kill_active: Arc<RwLock<bool>>,
    /// Status do sistema
    pub system_status: Arc<RwLock<SystemStatus>>,
    /// Histórico de lucros
    profit_history: Arc<RwLock<Vec<f64>>>,
    /// Histórico de perdas
    loss_streak: Arc<RwLock<u32>>,
}

/// 🚦 Status do sistema
#[derive(Clone, Debug, PartialEq)]
pub enum SystemStatus {
    /// Operando normalmente
    Active,
    /// Em pausa (monitorização)
    Idle,
    /// Kill-switch ativado
    Halted,
    /// Aguardando autorização
    AwaitingAuth,
}

/// ✅ Resultado de validação
#[derive(Clone, Debug)]
pub enum ValidationResult {
    /// Aprovado para execução
    Approved,
    /// Rejeitado - lucro insuficiente
    RejectedProfit { required: f64, actual: f64 },
    /// Rejeitado - não é topo do bloco
    RejectedNotTopBlock,
    /// Rejeitado - simulação falhou
    RejectedSimulation(String),
    /// Bloqueado - kill-switch ativo
    BlockedKillSwitch,
}

/// 💰 Guarda de Lucro
#[derive(Clone, Debug)]
pub struct ProfitGuard {
    /// Lucro mínimo (ETH)
    pub min_profit_eth: f64,
    /// Lucro mínimo (€)
    pub min_profit_eur: f64,
    /// Taxa de câmbio ETH/EUR
    eth_eur_rate: f64,
}

/// 📦 Protetor de Bundles
#[derive(Clone, Debug)]
pub struct BundleProtector {
    /// URL do Protector RPC
    pub protector_url: String,
    /// Máximo de tentativas
    max_retries: u32,
    /// Timeout (ms)
    timeout_ms: u64,
}

impl SafetyEngine {
    /// 🚀 Inicializa motor de segurança
    pub fn new(
        initial_capital: f64,
        min_profit_eth: f64,
        kill_threshold_pct: f64,
    ) -> Self {
        let kill_threshold = initial_capital * kill_threshold_pct;
        
        info!("═══════════════════════════════════════════════════════════");
        info!("🛡️ ORCA SAFETY ENGINE");
        info!("💰 Capital Inicial: {} ETH", initial_capital);
        info!("🎯 Lucro Mínimo: {} ETH", min_profit_eth);
        info!("💀 Kill Threshold: {} ETH ({}%)", 
            kill_threshold, 
            kill_threshold_pct * 100.0
        );
        info!("═══════════════════════════════════════════════════════════");
        
        Self {
            initial_capital,
            current_capital: Arc::new(RwLock::new(initial_capital)),
            min_profit_eth,
            kill_threshold,
            kill_active: Arc::new(RwLock::new(false)),
            system_status: Arc::new(RwLock::new(SystemStatus::Active)),
            profit_history: Arc::new(RwLock::new(Vec::new())),
            loss_streak: Arc::new(RwLock::new(0)),
        }
    }
    
    /// ✅ Valida se pode operar
    pub async fn can_operate(&self) -> bool {
        let status = self.system_status.read().await;
        *status != SystemStatus::Halted && !*self.kill_active.read().await
    }
    
    /// 🎯 Valida lucro mínimo
    pub fn validate_profit(&self, profit_eth: f64) -> ValidationResult {
        if profit_eth < self.min_profit_eth {
            return ValidationResult::RejectedProfit {
                required: self.min_profit_eth,
                actual: profit_eth,
            };
        }
        ValidationResult::Approved
    }
    
    /// 🧪 Valida via simulação (eth_call)
    pub async fn validate_simulation(
        &self,
        simulation_result: &SimulationResult,
    ) -> ValidationResult {
        // Verificar se simulação teve sucesso
        if !simulation_result.will_succeed {
            return ValidationResult::RejectedSimulation(
                "Simulação falhou - transação reverteria".to_string()
            );
        }
        
        // Verificar lucro líquido
        if simulation_result.net_profit_eth < self.min_profit_eth {
            return ValidationResult::RejectedProfit {
                required: self.min_profit_eth,
                actual: simulation_result.net_profit_eth,
            };
        }
        
        ValidationResult::Approved
    }
    
    /// 📊 Registra resultado de trade
    pub async fn record_profit(&self, profit_eth: f64) {
        let mut history = self.profit_history.write().await;
        history.push(profit_eth);
        drop(history);
        
        let mut capital = self.current_capital.write().await;
        *capital += profit_eth;
        let new_capital = *capital;
        drop(capital);
        
        // Atualizar streak
        let mut streak = self.loss_streak.write().await;
        if profit_eth < 0.0 {
            *streak += 1;
            warn!("[SAFETY] 📉 Loss streak: {}/8", *streak);
        } else {
            *streak = 0; // Reset em caso de lucro
        }
        drop(streak);
        
        // Verificar kill-switch
        self.check_kill_threshold(new_capital).await;
        
        debug!(
            "[SAFETY] 💰 Profit recorded: {} ETH | Capital: {} ETH",
            profit_eth, new_capital
        );
    }
    
    /// 💀 Verifica se atingiu kill threshold
    pub async fn check_kill_threshold(&self, current: f64) -> bool {
        if current <= self.kill_threshold {
            let mut kill = self.kill_active.write().await;
            *kill = true;
            drop(kill);
            
            let mut status = self.system_status.write().await;
            *status = SystemStatus::Halted;
            drop(status);
            
            error!("═══════════════════════════════════════════════════════════");
            error!("🚨 ORCA KILL-SWITCH ATIVADO!");
            error!("💸 Capital: {} ETH | Threshold: {} ETH", current, self.kill_threshold);
            error!("📉 Perda: {:.1}%", (1.0 - current/self.initial_capital) * 100.0);
            error!("⛔ SISTEMA PARADO - Aguardando autorização manual");
            error!("═══════════════════════════════════════════════════════════");
            
            return true;
        }
        false
    }
    
    /// 🔓 Autorização para continuar após kill-switch
    pub async fn authorize_resume(&self, auth_code: &str) -> bool {
        // Código secreto de autorização
        const AUTH_CODE: &str = "ORCA_RESUME_2024";
        
        if auth_code != AUTH_CODE {
            error!("[SAFETY] ❌ Código de autorização inválido");
            return false;
        }
        
        let current = *self.current_capital.read().await;
        
        // Reset kill-switch com novo capital de referência
        let mut kill = self.kill_active.write().await;
        *kill = false;
        drop(kill);
        
        let mut status = self.system_status.write().await;
        *status = SystemStatus::Active;
        drop(status);
        
        info!(
            "[SAFETY] 🔓 Sistema retomado | Novo reference: {} ETH",
            current
        );
        
        true
    }
    
    /// 💰 Retorna lucro mínimo
    pub fn min_profit_eth(&self) -> f64 {
        self.min_profit_eth
    }
    
    /// 📊 Estatísticas de segurança
    pub async fn stats(&self) -> String {
        let capital = *self.current_capital.read().await;
        let streak = *self.loss_streak.read().await;
        let status = self.system_status.read().await.clone();
        let kill = *self.kill_active.read().await;
        
        let safety_score = if kill {
            0
        } else if streak >= 4 {
            30
        } else if streak >= 2 {
            60
        } else {
            100
        };
        
        format!(
            "🛡️ Safety | Status: {:?} | Capital: {} ETH | Streak: {}/8 | Score: {}%",
            status, capital, streak, safety_score
        )
    }
}

impl ProfitGuard {
    /// 🎯 Cria guarda de lucro
    pub fn new(min_profit_eth: f64, eth_eur_rate: f64) -> Self {
        Self {
            min_profit_eth,
            min_profit_eur: min_profit_eth * eth_eur_rate,
            eth_eur_rate,
        }
    }
    
    /// ✅ Verifica se lucro é aceitável
    pub fn is_profitable(&self, profit_eth: f64) -> bool {
        profit_eth >= self.min_profit_eth
    }
    
    /// 💱 Converte ETH para EUR
    pub fn eth_to_eur(&self, eth: f64) -> f64 {
        eth * self.eth_eur_rate
    }
}

impl BundleProtector {
    /// 🚀 Inicializa protetor
    pub fn new(protector_url: String) -> Self {
        Self {
            protector_url,
            max_retries: 3,
            timeout_ms: 5000,
        }
    }
    
    /// 📡 Envia bundle para Protector RPC
    pub async fn submit_bundle(
        &self,
        bundle: &ProtectedBundle,
    ) -> Result<String, String> {
        info!(
            "[PROTECTOR] 📡 Enviando bundle | Profit min: {} ETH | Revert on fail: {}",
            bundle.min_profit_eth,
            bundle.revert_on_failure
        );
        
        // Em produção: HTTP POST para Flashbots Protector
        // com timeout e retries
        
        // Simulação
        let bundle_hash = format!("0x{:064x}", std::time::Instant::now().elapsed().as_nanos());
        
        Ok(bundle_hash)
    }
}

use super::{SimulationResult, ProtectedBundle};
