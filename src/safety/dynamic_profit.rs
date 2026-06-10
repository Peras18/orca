//! DYNAMIC PROFIT THRESHOLD
//! Começa em 5€ e desce até 2€ se não houver trades em 4 horas
//! Adapta-se à frequência de trading para manter capital trabalhando

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use std::sync::Arc;

/// 🎯 Motor de Profit Adaptativo
#[derive(Clone, Debug)]
pub struct ProfitAdaptiveEngine {
    /// Threshold atual (€)
    current_threshold: Arc<RwLock<f64>>,
    /// Threshold máximo (€)
    max_threshold: f64,
    /// Threshold mínimo (€)
    min_threshold: f64,
    /// Timestamp do último trade
    last_trade_time: Arc<RwLock<u64>>,
    /// Tempo sem trades para ativar descida (4 horas = 14400 segundos)
    idle_threshold_seconds: u64,
    /// Taxa de descida (€/hora sem trades)
    decay_rate_per_hour: f64,
    /// Contador de trades hoje
    daily_trade_count: Arc<RwLock<u32>>,
    /// Timestamp de início do dia
    day_start: Arc<RwLock<u64>>,
    /// Histórico de thresholds
    threshold_history: Arc<RwLock<Vec<(u64, f64)>>>, // (timestamp, threshold)
}

/// 📊 Estado adaptativo
#[derive(Clone, Debug)]
pub struct AdaptiveState {
    /// Threshold atual
    pub threshold: f64,
    /// Tempo desde último trade (segundos)
    pub seconds_since_last_trade: u64,
    /// Trades hoje
    pub trades_today: u32,
    /// Tendência (subindo/descendo/estável)
    pub trend: ThresholdTrend,
}

/// 📈 Tendência do threshold
#[derive(Clone, Debug, PartialEq)]
pub enum ThresholdTrend {
    /// Subindo (muitos trades)
    Rising,
    /// Descendo (poucos/nenhum trade)
    Falling,
    /// Estável
    Stable,
}

impl ProfitAdaptiveEngine {
    /// 🚀 Inicializa motor adaptativo
    /// 
    /// # Arguments
    /// * `capital_eur` - Capital inicial (usado para calibrar thresholds)
    pub fn new(capital_eur: f64) -> Self {
        let max_threshold = 5.0; // 5€
        let min_threshold = 2.0;   // 2€
        
        // Calibrar baseado no capital
        let calculated_max = capital_eur * 0.0625;
        let calculated_min = capital_eur * 0.025;
        
        let calibrated_max = f64::max(f64::min(calculated_max, max_threshold), 3.0); // 6.25% do capital, max 5€
        let calibrated_min = f64::max(f64::min(calculated_min, min_threshold), 1.5);  // 2.5% do capital, min 2€
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        info!("═══════════════════════════════════════════════════════════");
        info!("🎯 PROFIT ADAPTIVE ENGINE - Threshold Dinâmico");
        info!("💰 Max Threshold: {}€ (capital: {}€)", calibrated_max, capital_eur);
        info!("💰 Min Threshold: {}€", calibrated_min);
        info!("⏱️  Idle Trigger: 4 horas sem trades");
        info!("📉 Decay Rate: 0.75€/hora");
        info!("📈 Iniciando em: {}€", calibrated_max);
        info!("═══════════════════════════════════════════════════════════");
        
        let mut history = Vec::new();
        history.push((now, calibrated_max));
        
        Self {
            current_threshold: Arc::new(RwLock::new(calibrated_max)),
            max_threshold: calibrated_max,
            min_threshold: calibrated_min,
            last_trade_time: Arc::new(RwLock::new(now)),
            idle_threshold_seconds: 4 * 3600, // 4 horas
            decay_rate_per_hour: 0.75, // Desce 0.75€ por hora
            daily_trade_count: Arc::new(RwLock::new(0)),
            day_start: Arc::new(RwLock::new(now)),
            threshold_history: Arc::new(RwLock::new(history)),
        }
    }
    
    /// 📊 Retorna threshold atual
    pub async fn current_threshold(&self) -> f64 {
        *self.current_threshold.read().await
    }
    
    /// 🔔 Notifica que trade foi executado
    pub async fn notify_trade_executed(&self, profit_eur: f64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        // Atualizar último trade
        *self.last_trade_time.write().await = now;
        
        // Incrementar contador diário
        let mut count = self.daily_trade_count.write().await;
        *count += 1;
        drop(count);
        
        // Resetar threshold para máximo (estamos ativos!)
        let mut threshold = self.current_threshold.write().await;
        let old_value = *threshold;
        *threshold = self.max_threshold;
        drop(threshold);
        
        if old_value < self.max_threshold {
            info!(
                "[ADAPTIVE] 📈 THRESHOLD RESETADO | Trade com {}€ | Volta para {}€",
                profit_eur, self.max_threshold
            );
        }
        
        // Guardar no histórico
        self.threshold_history.write().await.push((now, self.max_threshold));
        
        trace!(
            "[ADAPTIVE] 🔔 Trade notificado | Profit: {}€ | Trades hoje: {}",
            profit_eur, *self.daily_trade_count.read().await
        );
    }
    
    /// ⏱️ Verifica e ajusta threshold (chamar periodicamente, ex: a cada 5 min)
    pub async fn update_threshold(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let last_trade = *self.last_trade_time.read().await;
        let idle_seconds = now - last_trade;
        
        // Se passaram 4 horas sem trades, descer threshold
        if idle_seconds >= self.idle_threshold_seconds {
            let hours_idle = idle_seconds as f64 / 3600.0;
            let decay = hours_idle * self.decay_rate_per_hour;
            
            let mut threshold = self.current_threshold.write().await;
            let old_value = *threshold;
            
            // Calcular novo threshold (não desce abaixo do mínimo)
            let new_value = f64::max(self.max_threshold - decay, self.min_threshold);
            *threshold = new_value;
            drop(threshold);
            
            if (old_value - new_value).abs() > 0.01 {
                warn!(
                    "[ADAPTIVE] 📉 THRESHOLD DESCENDO | {}h idle | {}€ → {}€ (min: {}€)",
                    hours_idle as u64,
                    old_value,
                    new_value,
                    self.min_threshold
                );
                
                // Guardar no histórico
                self.threshold_history.write().await.push((now, new_value));
            }
        }
        
        // Reset diário
        let day_start = *self.day_start.read().await;
        if now - day_start >= 86400 { // 24 horas
            *self.daily_trade_count.write().await = 0;
            *self.day_start.write().await = now;
            info!("[ADAPTIVE] 🌅 Novo dia - contador de trades resetado");
        }
    }
    
    /// 📊 Retorna estado adaptativo completo
    pub async fn adaptive_state(&self) -> AdaptiveState {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let last_trade = *self.last_trade_time.read().await;
        let seconds_since_last = now - last_trade;
        
        let current = *self.current_threshold.read().await;
        let trend = if current >= self.max_threshold * 0.95 {
            ThresholdTrend::Stable
        } else if current < self.max_threshold * 0.9 {
            ThresholdTrend::Falling
        } else {
            ThresholdTrend::Stable
        };
        
        AdaptiveState {
            threshold: current,
            seconds_since_last_trade: seconds_since_last,
            trades_today: *self.daily_trade_count.read().await,
            trend,
        }
    }
    
    /// 📈 Histórico de thresholds
    pub async fn threshold_history(&self) -> Vec<(u64, f64)> {
        self.threshold_history.read().await.clone()
    }
    
    /// 💡 Recomendação de ação
    pub async fn recommendation(&self) -> String {
        let state = self.adaptive_state().await;
        let threshold = self.current_threshold().await;
        
        if state.seconds_since_last_trade > self.idle_threshold_seconds {
            format!(
                "⏳ IDLE MODE | Threshold: {}€ (desceu de {}€) | Último trade há {}h | APROVEITAR OPORTUNIDADES MENORES",
                threshold,
                self.max_threshold,
                state.seconds_since_last_trade / 3600
            )
        } else if state.trades_today >= 10 {
            format!(
                "🚀 HIGH FREQUENCY | {} trades hoje | Threshold: {}€ | SISTEMA ATIVO",
                state.trades_today,
                threshold
            )
        } else {
            format!(
                "📊 NORMAL | Threshold: {}€ | Último trade há {}min | {} trades hoje",
                threshold,
                state.seconds_since_last_trade / 60,
                state.trades_today
            )
        }
    }
    
    /// 📊 Estatísticas
    pub async fn stats(&self) -> String {
        let state = self.adaptive_state().await;
        let hours_idle = state.seconds_since_last_trade as f64 / 3600.0;
        
        format!(
            "🎯 Adaptive | Threshold: {}€ ({}€→{}€) | Idle: {:.1}h | Trades: {} | Trend: {:?}",
            state.threshold,
            self.max_threshold,
            self.min_threshold,
            hours_idle,
            state.trades_today,
            state.trend
        )
    }
}

use tracing::{info, warn, trace};
