//! KILL-SWITCH - PROTEÇÃO DE CAPITAL
//! Para tudo se banca descer de 80€ para 40€ (50% de perda = 8 tentativas falhadas)
//! Proteção absoluta de capital acima de tudo

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use std::sync::Arc;

/// 🚨 Kill-Switch de Capital
#[derive(Clone, Debug)]
pub struct KillSwitch {
    /// Capital inicial de referência
    reference_capital: Arc<RwLock<f64>>,
    /// Kill threshold (50% do inicial)
    kill_threshold: Arc<RwLock<f64>>,
    /// Warning threshold (75% do inicial)
    warning_threshold: f64,
    /// Se já foi ativado
    triggered: Arc<RwLock<bool>>,
    /// Timestamp de ativação
    triggered_at: Arc<RwLock<Option<u64>>>,
    /// Histórico de perdas consecutivas
    loss_streak: Arc<RwLock<u32>>,
    /// Máximo de perdas consecutivas permitidas
    max_loss_streak: u32,
    /// Drawdown máximo permitido (%)
    max_drawdown_pct: f64,
}

/// 🎚️ Nível de Risco
#[derive(Clone, Debug, PartialEq)]
pub enum RiskLevel {
    /// Verde - seguro
    Safe,
    /// Amarelo - atenção
    Warning,
    /// Laranja - perigo
    Danger,
    /// Vermelho - halt
    Critical,
}

/// 📊 Estado do kill-switch
#[derive(Clone, Debug)]
pub struct KillSwitchState {
    /// Se está ativado
    pub triggered: bool,
    /// Capital atual
    pub current_capital: f64,
    /// Threshold de kill
    pub kill_threshold: f64,
    /// Drawdown atual (%)
    pub current_drawdown_pct: f64,
    /// Nível de risco
    pub risk_level: RiskLevel,
    /// Streak de perdas
    pub loss_streak: u32,
    /// Tempo desde ativação (se ativado)
    pub time_since_trigger: Option<u64>,
}

/// 💀 Alerta de Risco
#[derive(Clone, Debug)]
pub struct RiskAlert {
    /// Nível
    pub level: RiskLevel,
    /// Mensagem
    pub message: String,
    /// Ação recomendada
    pub recommended_action: String,
    /// Timestamp
    pub timestamp: u64,
}

impl KillSwitch {
    /// 🚀 Inicializa kill-switch
    /// 
    /// # Arguments
    /// * `initial_capital` - Capital inicial em €
    pub fn new(initial_capital: f64) -> Self {
        let kill_threshold = initial_capital * 0.5;   // 50% = 40€ se inicial = 80€
        let warning_threshold = initial_capital * 0.75; // 75% = 60€ se inicial = 80€
        
        info!("═══════════════════════════════════════════════════════════");
        info!("🚨 KILL-SWITCH INICIALIZADO");
        info!("💰 Capital Inicial: {}€", initial_capital);
        info!("⚠️  Warning (75%): {}€", warning_threshold);
        info!("💀 Kill (50%): {}€", kill_threshold);
        info!("📉 Max Drawdown: 50%");
        info!("🔥 Max Loss Streak: 8 tentativas");
        info!("═══════════════════════════════════════════════════════════");
        
        Self {
            reference_capital: Arc::new(RwLock::new(initial_capital)),
            kill_threshold: Arc::new(RwLock::new(kill_threshold)),
            warning_threshold,
            triggered: Arc::new(RwLock::new(false)),
            triggered_at: Arc::new(RwLock::new(None)),
            loss_streak: Arc::new(RwLock::new(0)),
            max_loss_streak: 8, // ~8 tentativas a -5€ cada = -40€
            max_drawdown_pct: 0.50,
        }
    }
    
    /// ✅ Verifica se pode operar
    pub async fn can_operate(&self) -> bool {
        !*self.triggered.read().await
    }
    
    /// 🎯 Verifica se atingiu threshold
    pub async fn check_threshold(&self, current_capital: f64) -> bool {
        let kill = *self.kill_threshold.read().await;
        let mut triggered = self.triggered.write().await;
        
        if current_capital <= kill && !*triggered {
            *triggered = true;
            *self.triggered_at.write().await = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            );
            
            return true; // Kill-switch ATIVADO
        }
        
        false
    }
    
    /// 📉 Registra resultado de trade
    pub async fn record_trade_result(&self, profit_loss_eur: f64) {
        let mut streak = self.loss_streak.write().await;
        
        if profit_loss_eur < 0.0 {
            *streak += 1;
            
            let current_streak = *streak;
            drop(streak);
            
            if current_streak >= self.max_loss_streak {
                error!(
                    "🚨 KILL-SWITCH TRIGGERED | {} perdas consecutivas | Capital em risco",
                    current_streak
                );
                *self.triggered.write().await = true;
            } else if current_streak >= self.max_loss_streak / 2 {
                warn!(
                    "⚠️  ATENÇÃO | {} perdas consecutivas | Streak perigosa",
                    current_streak
                );
            }
        } else {
            *streak = 0; // Reset em caso de lucro
            drop(streak);
        }
        
        trace!(
            "[KILL-SWITCH] Trade result: {:+.2}€ | Streak: {}",
            profit_loss_eur,
            *self.loss_streak.read().await
        );
    }
    
    /// 🎚️ Avalia nível de risco atual
    pub async fn assess_risk(&self, current_capital: f64) -> RiskLevel {
        let reference = *self.reference_capital.read().await;
        let drawdown = (reference - current_capital) / reference;
        
        if *self.triggered.read().await {
            RiskLevel::Critical
        } else if drawdown >= 0.50 {
            RiskLevel::Critical
        } else if drawdown >= 0.35 {
            RiskLevel::Danger
        } else if drawdown >= 0.25 || current_capital <= self.warning_threshold {
            RiskLevel::Warning
        } else {
            RiskLevel::Safe
        }
    }
    
    /// 🔔 Gera alerta se necessário
    pub async fn check_and_alert(&self, current_capital: f64) -> Option<RiskAlert> {
        let risk = self.assess_risk(current_capital).await;
        let reference = *self.reference_capital.read().await;
        let drawdown = (reference - current_capital) / reference * 100.0;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        match risk {
            RiskLevel::Critical => Some(RiskAlert {
                level: RiskLevel::Critical,
                message: format!(
                    "🚨 KILL-SWITCH ATIVADO! Capital caiu de {}€ para {}€ (perda de {:.1}%)",
                    reference, current_capital, drawdown
                ),
                recommended_action: "SISTEMA PARADO. Aguardando autorização manual.".to_string(),
                timestamp: now,
            }),
            RiskLevel::Danger => Some(RiskAlert {
                level: RiskLevel::Danger,
                message: format!(
                    "⚠️ DRAWDOWN CRÍTICO: {:.1}% | Capital: {}€ | Perda total: {}€",
                    drawdown,
                    current_capital,
                    reference - current_capital
                ),
                recommended_action: "REDUZIR TAMANHO DOS TRADES IMEDIATAMENTE".to_string(),
                timestamp: now,
            }),
            RiskLevel::Warning => Some(RiskAlert {
                level: RiskLevel::Warning,
                message: format!(
                    "⚡ Atenção: Drawdown de {:.1}% | {}€ abaixo do reference",
                    drawdown,
                    reference - current_capital
                ),
                recommended_action: "Monitorizar de perto. Considerar pausa.".to_string(),
                timestamp: now,
            }),
            RiskLevel::Safe => None,
        }
    }
    
    /// 🔓 Reset após autorização
    pub async fn reset(&self, new_reference_capital: f64) {
        let mut ref_cap = self.reference_capital.write().await;
        *ref_cap = new_reference_capital;
        drop(ref_cap);
        
        let mut kill = self.kill_threshold.write().await;
        *kill = new_reference_capital * 0.5;
        drop(kill);
        
        *self.triggered.write().await = false;
        *self.triggered_at.write().await = None;
        *self.loss_streak.write().await = 0;
        
        info!(
            "[KILL-SWITCH] 🔓 RESET | Novo reference: {}€ | Kill: {}€ | Sistema retomado",
            new_reference_capital,
            new_reference_capital * 0.5
        );
    }
    
    /// 📊 Estado completo
    pub async fn state(&self, current_capital: f64) -> KillSwitchState {
        let reference = *self.reference_capital.read().await;
        let drawdown = (reference - current_capital) / reference * 100.0;
        let triggered = *self.triggered.read().await;
        
        let time_since = if triggered {
            self.triggered_at.read().await.map(|t| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() - t
            })
        } else {
            None
        };
        
        KillSwitchState {
            triggered,
            current_capital,
            kill_threshold: *self.kill_threshold.read().await,
            current_drawdown_pct: drawdown,
            risk_level: self.assess_risk(current_capital).await,
            loss_streak: *self.loss_streak.read().await,
            time_since_trigger: time_since,
        }
    }
    
    /// 💀 Retorna threshold de kill
    pub async fn kill_threshold(&self) -> f64 {
        *self.kill_threshold.read().await
    }
    
    /// 📊 Estatísticas
    pub async fn stats(&self, current_capital: f64) -> String {
        let state = self.state(current_capital).await;
        
        let status = if state.triggered {
            "💀 HALTED"
        } else {
            match state.risk_level {
                RiskLevel::Safe => "🟢 SAFE",
                RiskLevel::Warning => "🟡 WARNING",
                RiskLevel::Danger => "🟠 DANGER",
                RiskLevel::Critical => "🔴 CRITICAL",
            }
        };
        
        format!(
            "{} | Capital: {}€/{}€ | Drawdown: {:.1}% | Streak: {}/{}",
            status,
            current_capital,
            state.kill_threshold,
            state.current_drawdown_pct,
            state.loss_streak,
            self.max_loss_streak
        )
    }
}

/// 🛡️ Guarda de Capital (wrapper de alto nível)
pub struct CapitalGuard {
    kill_switch: KillSwitch,
}

impl CapitalGuard {
    pub fn new(initial_capital: f64) -> Self {
        Self {
            kill_switch: KillSwitch::new(initial_capital),
        }
    }
    
    /// ✅ Verifica se operação é segura
    pub async fn validate_operation(&self, _current_capital: f64, risk_amount: f64) -> bool {
        // Verificar kill-switch
        if !self.kill_switch.can_operate().await {
            return false;
        }
        
        // Verificar se risco não excede limites
        let reference = self.kill_switch.reference_capital.read().await;
        let risk_pct = risk_amount / *reference;
        
        risk_pct < 0.125 // Máximo 12.5% do capital por trade
    }
}

use tracing::{info, error, warn, trace};
