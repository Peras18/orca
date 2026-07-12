//! 🔒 Circuit Breaker — Proteção contra perdas em sequência
//! 
//! Se 5 txns falharem em 10 minutos, pausa automática por 30 minutos.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;
use tracing::{error, warn, info};
use tokio::time::{interval, Duration, Instant};

/// Estados do circuit breaker
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CircuitState {
    Closed,     // Normal operation
    Open,       // Pausado - não executar
    HalfOpen,   // Testando recuperação
}

/// Circuit breaker com janela deslizante
pub struct CircuitBreaker {
    /// Estado atual
    state: Arc<RwLock<CircuitState>>,
    /// Contador de falhas na janela
    failures: Arc<RwLock<Vec<Instant>>>,
    /// Threshold de falhas para abrir circuito
    failure_threshold: usize,
    /// Janela de tempo para contar falhas (10 min)
    window_duration: Duration,
    /// Duração do circuito aberto (30 min)
    open_duration: Duration,
    /// Contador total de falhas
    total_failures: AtomicU64,
    /// Última vez que o circuito abriu
    last_opened: Arc<RwLock<Option<Instant>>>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failures: Arc::new(RwLock::new(Vec::new())),
            failure_threshold: 5,
            window_duration: Duration::from_secs(600), // 10 min
            open_duration: Duration::from_secs(1800),  // 30 min
            total_failures: AtomicU64::new(0),
            last_opened: Arc::new(RwLock::new(None)),
        }
    }

    /// Verifica se pode executar
    pub fn can_execute(&self) -> bool {
        let state = *self.state.read();
        
        match state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Verificar se já passou o tempo de pausa
                let last = *self.last_opened.read();
                if let Some(opened) = last {
                    if opened.elapsed() >= self.open_duration {
                        // Tentar meio aberto
                        *self.state.write() = CircuitState::HalfOpen;
                        info!("🔒 CircuitBreaker: Half-open, testing...");
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Regista uma falha
    pub fn record_failure(&self) {
        let now = Instant::now();
        
        {
            let mut failures = self.failures.write();
            failures.push(now);
            
            // Limpar falhas antigas fora da janela
            failures.retain(|&f| f.elapsed() < self.window_duration);
        }
        
        self.total_failures.fetch_add(1, Ordering::SeqCst);
        
        let failure_count = self.failures.read().len();
        
        if failure_count >= self.failure_threshold {
            let mut state = self.state.write();
            if *state == CircuitState::Closed {
                *state = CircuitState::Open;
                *self.last_opened.write() = Some(now);
                error!(
                    "🔒 CircuitBreaker OPENED! {} failures in 10min | Pausing 30min",
                    failure_count
                );
            }
        } else {
            warn!(
                "🔒 CircuitBreaker: {} failures (threshold: {})",
                failure_count, self.failure_threshold
            );
        }
    }

    /// Regista um sucesso (no modo half-open, fecha o circuito)
    pub fn record_success(&self) {
        let state = *self.state.read();
        
        if state == CircuitState::HalfOpen {
            *self.state.write() = CircuitState::Closed;
            self.failures.write().clear();
            info!("🔒 CircuitBreaker CLOSED - operations resumed");
        }
    }

    /// Força pausa manual
    pub fn force_pause(&self) {
        *self.state.write() = CircuitState::Open;
        *self.last_opened.write() = Some(Instant::now());
        error!("🔒 CircuitBreaker: MANUAL PAUSE activated");
    }

    /// Resume manual
    pub fn force_resume(&self) {
        *self.state.write() = CircuitState::Closed;
        self.failures.write().clear();
        info!("🔒 CircuitBreaker: MANUAL RESUME");
    }

    /// Estado atual
    pub fn current_state(&self) -> CircuitState {
        *self.state.read()
    }

    /// Estatísticas
    pub fn stats(&self) -> BreakerStats {
        BreakerStats {
            state: *self.state.read(),
            recent_failures: self.failures.read().len(),
            total_failures: self.total_failures.load(Ordering::SeqCst),
            last_opened: *self.last_opened.read(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BreakerStats {
    pub state: CircuitState,
    pub recent_failures: usize,
    pub total_failures: u64,
    pub last_opened: Option<Instant>,
}

/// Task de background para limpeza periódica
pub async fn run_circuit_maintenance(breaker: Arc<CircuitBreaker>) {
    let mut ticker = interval(Duration::from_secs(60)); // A cada minuto
    
    loop {
        ticker.tick().await;
        
        // Limpar falhas antigas
        {
            let mut failures = breaker.failures.write();
            let before = failures.len();
            failures.retain(|&f| f.elapsed() < breaker.window_duration);
            let removed = before - failures.len();
            
            if removed > 0 {
                info!("🔒 CircuitBreaker: Cleaned {} old failures", removed);
            }
        }
        
        // Tentar recuperar de Open para HalfOpen
        {
            let state = *breaker.state.read();
            let last = *breaker.last_opened.read();
            
            if state == CircuitState::Open {
                if let Some(opened) = last {
                    if opened.elapsed() >= breaker.open_duration {
                        *breaker.state.write() = CircuitState::HalfOpen;
                        info!("🔒 CircuitBreaker: Auto-recovery to HalfOpen");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_initially_closed() {
        let breaker = CircuitBreaker::new();
        assert_eq!(breaker.current_state(), CircuitState::Closed);
        assert!(breaker.can_execute());
    }

    #[test]
    fn test_circuit_opens_after_failures() {
        let breaker = CircuitBreaker::new();
        
        // 5 falhas devem abrir o circuito
        for _ in 0..5 {
            breaker.record_failure();
        }
        
        assert_eq!(breaker.current_state(), CircuitState::Open);
        assert!(!breaker.can_execute());
    }
}
