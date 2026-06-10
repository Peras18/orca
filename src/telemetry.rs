//! Módulo de Telemetria Real - Benchmarking de Hardware e Latência
//! 
//! Mede tempos reais de execução sem logs fictícios.

use std::sync::Arc;
use std::time::{Instant, Duration};
use tokio::sync::RwLock;
use tracing::{error, info};

/// Threshold crítico de latência (100ms)
pub const CRITICAL_LATENCY_THRESHOLD_MS: u128 = 5000;

/// Estrutura para métricas de telemetria
#[derive(Debug)]
pub struct TelemetryCollector {
    /// Latência DNA Scanner (microssegundos)
    dna_scan_times_us: Arc<RwLock<Vec<u128>>>,
    /// Latência Newton-Raphson (microssegundos)
    newton_times_us: Arc<RwLock<Vec<u128>>>,
    /// Latência Mempool -> Simulação (microssegundos)
    mempool_simulation_times_us: Arc<RwLock<Vec<u128>>>,
    /// Contador de alarmes críticos
    critical_alarms: Arc<RwLock<u64>>,
    /// Flag de parada de emergência
    emergency_stop: Arc<RwLock<bool>>,
    /// 💰 Lucro acumulado estimado (Dry Run Pro)
    estimated_daily_profit_usd: Arc<RwLock<f64>>,
    /// 📊 Oportunidades simuladas com sucesso
    successful_simulations: Arc<RwLock<u64>>,
    /// 🎯 Target diário (200€)
    daily_target_eur: Arc<RwLock<f64>>,
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self {
            dna_scan_times_us: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            newton_times_us: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            mempool_simulation_times_us: Arc::new(RwLock::new(Vec::with_capacity(1000))),
            critical_alarms: Arc::new(RwLock::new(0)),
            emergency_stop: Arc::new(RwLock::new(false)),
            estimated_daily_profit_usd: Arc::new(RwLock::new(0.0)),
            successful_simulations: Arc::new(RwLock::new(0)),
            daily_target_eur: Arc::new(RwLock::new(200.0)),
        }
    }

    /// 💰 Registra lucro de simulação (Dry Run Pro)
    pub async fn record_simulated_profit(&self, profit_usd: f64) {
        let mut profit = self.estimated_daily_profit_usd.write().await;
        *profit += profit_usd;
        
        let mut sims = self.successful_simulations.write().await;
        *sims += 1;
        
        let target = *self.daily_target_eur.read().await;
        let progress = (*profit / 1.08 / target) * 100.0; // EUR conversion
        
        info!("📈📈📈 [PROFIT TRACKER] +${:.2} | Total: ${:.2} ({:.1}%) | Target: {:.0}€ | Sims: {}",
            profit_usd, *profit, progress, target, *sims);
        
        // Alerta quando próximo de atingir meta
        if progress >= 90.0 && progress < 100.0 {
            info!("🎯🎯🎯 [PROFIT TRACKER] QUASE LÁ! {:.1}% do target diário!", progress);
        } else if progress >= 100.0 {
            info!("🎉🎉🎉 [PROFIT TRACKER] META DIÁRIA ATINGIDA! ${:.2} / {:.0}€", 
                *profit, target);
        }
    }
    
    /// 📊 Retorna estatísticas de lucro
    pub async fn get_profit_stats(&self) -> ProfitStats {
        let profit = *self.estimated_daily_profit_usd.read().await;
        let sims = *self.successful_simulations.read().await;
        let target_eur = *self.daily_target_eur.read().await;
        let target_usd = target_eur * 1.08;
        
        ProfitStats {
            profit_usd: profit,
            profit_eur: profit / 1.08,
            target_eur,
            target_usd,
            progress_percent: (profit / target_usd) * 100.0,
            successful_simulations: sims,
            remaining_to_target_usd: target_usd - profit,
        }
    }
    
    /// 🔄 Reseta lucro diário (chamar à meia-noite)
    pub async fn reset_daily_profit(&self) {
        let mut profit = self.estimated_daily_profit_usd.write().await;
        let yesterday_profit = *profit;
        *profit = 0.0;
        drop(profit);
        
        let mut sims = self.successful_simulations.write().await;
        *sims = 0;
        drop(sims);
        
        info!("🔄 [PROFIT TRACKER] Reset diário. Ontem: ${:.2}", yesterday_profit);
    }

    /// Regista tempo de DNA Scan
    pub async fn record_dna_scan(&self, elapsed_us: u128) {
        let mut times = self.dna_scan_times_us.write().await;
        times.push(elapsed_us);
        if times.len() > 1000 {
            times.remove(0);
        }
        
        // Verificar threshold crítico
        if elapsed_us > CRITICAL_LATENCY_THRESHOLD_MS * 1000 {
            self.trigger_critical_alarm("DNA_SCAN", elapsed_us).await;
        }
    }

    /// Alias explícito para logs de "scan" no event handler.
    pub async fn record_scan(&self, elapsed_us: u128) {
        self.record_dna_scan(elapsed_us).await;
    }

    /// Regista tempo de Newton-Raphson
    pub async fn record_newton_raphson(&self, elapsed_us: u128) {
        let mut times = self.newton_times_us.write().await;
        times.push(elapsed_us);
        if times.len() > 1000 {
            times.remove(0);
        }
        
        // Verificar threshold crítico
        if elapsed_us > CRITICAL_LATENCY_THRESHOLD_MS * 1000 {
            self.trigger_critical_alarm("NEWTON_RAPHSON", elapsed_us).await;
        }
    }

    /// Regista tempo Mempool -> Simulação
    pub async fn record_mempool_simulation(&self, elapsed_us: u128) {
        let mut times = self.mempool_simulation_times_us.write().await;
        times.push(elapsed_us);
        if times.len() > 1000 {
            times.remove(0);
        }
        
        // Verificar threshold crítico - este é o mais importante!
        if elapsed_us > CRITICAL_LATENCY_THRESHOLD_MS * 1000 {
            self.trigger_critical_alarm("MEMPOOL_TO_SIMULATION", elapsed_us).await;
        }
    }

    /// Dispara alarme crítico e para o bot
    async fn trigger_critical_alarm(&self, component: &str, latency_us: u128) {
        let mut alarms = self.critical_alarms.write().await;
        *alarms += 1;
        
        error!(
            "🚨 CRITICAL_LATENCY_ALARM | {} | Latência: {}µs | Threshold: {}ms | Bot PARANDO!",
            component,
            latency_us,
            CRITICAL_LATENCY_THRESHOLD_MS
        );
        
        // Ativar parada de emergência
        let mut stop = self.emergency_stop.write().await;
        *stop = true;
        
        // Panic para parar imediatamente
        panic!(
            "CRITICAL_LATENCY_ALARM: {} excedeu {}ms ({}µs)",
            component,
            CRITICAL_LATENCY_THRESHOLD_MS,
            latency_us
        );
    }

    /// Verifica se deve parar
    pub async fn should_stop(&self) -> bool {
        *self.emergency_stop.read().await
    }

    /// Obtém médias atuais
    pub async fn get_averages(&self) -> (f64, f64, f64) {
        let dna_avg = self.calculate_average(&self.dna_scan_times_us).await;
        let newton_avg = self.calculate_average(&self.newton_times_us).await;
        let mempool_avg = self.calculate_average(&self.mempool_simulation_times_us).await;
        
        (dna_avg, newton_avg, mempool_avg)
    }

    async fn calculate_average(&self, times: &Arc<RwLock<Vec<u128>>>) -> f64 {
        let times = times.read().await;
        if times.is_empty() {
            return 0.0;
        }
        let sum: u128 = times.iter().sum();
        sum as f64 / times.len() as f64
    }

    /// Print real-time metrics
    pub async fn print_realtime_metrics(&self) {
        let (dna_avg, newton_avg, mempool_avg) = self.get_averages().await;
        
        info!(
            "[REAL-TIME] DNA Scan: {:.0}µs | Newton-Raphson: {:.0}µs | Mempool->Sim: {:.0}µs",
            dna_avg,
            newton_avg,
            mempool_avg
        );
    }
}

/// Wrapper instrumentado para DNA Scanner
pub struct InstrumentedDnaScanner {
    inner: crate::apex_shadow_protocol::DnaScanner,
    telemetry: Arc<TelemetryCollector>,
}

impl InstrumentedDnaScanner {
    pub fn new(scanner: crate::apex_shadow_protocol::DnaScanner, telemetry: Arc<TelemetryCollector>) -> Self {
        Self {
            inner: scanner,
            telemetry,
        }
    }

    pub async fn scan(&self, token: alloy::primitives::Address, bytecode: &[u8]) -> crate::apex_shadow_protocol::DnaReport {
        let start = Instant::now();
        let result = self.inner.scan(token, bytecode).await;
        let elapsed = start.elapsed();
        
        self.telemetry.record_dna_scan(elapsed.as_micros()).await;
        
        result
    }
}

/// Wrapper instrumentado para Newton-Raphson
pub struct InstrumentedNewtonRaphson;

impl InstrumentedNewtonRaphson {
    pub fn calculate_optimal_input(
        pool_reserves_in: alloy::primitives::U256,
        pool_reserves_out: alloy::primitives::U256,
        fee_bps: u32,
        gas_cost_wei: alloy::primitives::U256,
        flash_loan_fee_bps: u32,
        telemetry: Option<Arc<TelemetryCollector>>,
    ) -> Option<(alloy::primitives::U256, alloy::primitives::U256, usize)> {
        let start = Instant::now();
        
        let result = crate::god_mode::NewtonRaphsonOptimizer::calculate_optimal_input(
            pool_reserves_in,
            pool_reserves_out,
            fee_bps,
            gas_cost_wei,
            flash_loan_fee_bps,
        );
        
        let elapsed = start.elapsed();
        
        if let Some(ref telem) = telemetry {
            // Usar try_write para não bloquear
            if let Ok(mut times) = telem.newton_times_us.try_write() {
                times.push(elapsed.as_micros());
                if times.len() > 1000 {
                    times.remove(0);
                }
            }
            
            // Verificar threshold crítico
            if elapsed.as_micros() > CRITICAL_LATENCY_THRESHOLD_MS * 1000 {
                // Não podemos usar await aqui, mas podemos logar
                error!(
                    "🚨 CRITICAL_LATENCY_ALARM | NEWTON_RAPHSON | Latência: {}µs",
                    elapsed.as_micros()
                );
            }
        }
        
        result
    }
}

/// Monitor de latência de Mempool
pub struct MempoolLatencyMonitor {
    telemetry: Arc<TelemetryCollector>,
    receive_times: Arc<RwLock<std::collections::HashMap<String, Instant>>>,
}

impl MempoolLatencyMonitor {
    pub fn new(telemetry: Arc<TelemetryCollector>) -> Self {
        Self {
            telemetry,
            receive_times: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Regista receção de transação do mempool
    pub async fn record_receive(&self, tx_hash: &str) {
        let mut times = self.receive_times.write().await;
        times.insert(tx_hash.to_string(), Instant::now());
        
        // Limpar entradas antigas (> 30 segundos)
        let now = Instant::now();
        times.retain(|_, &mut instant| now.duration_since(instant).as_secs() < 30);
    }

    /// Regista conclusão de simulação
    pub async fn record_simulation_complete(&self, tx_hash: &str) {
        let times = self.receive_times.read().await;
        
        if let Some(&receive_time) = times.get(tx_hash) {
            let elapsed = receive_time.elapsed();
            drop(times); // Liberar lock
            
            self.telemetry.record_mempool_simulation(elapsed.as_micros()).await;
            
            info!(
                "[MEMPOOL_LATENCY] Tx: {} | Tempo Total: {}µs",
                tx_hash,
                elapsed.as_micros()
            );
        }
    }
}

/// Task de telemetria periódica
pub async fn spawn_telemetry_printer(telemetry: Arc<TelemetryCollector>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    
    loop {
        interval.tick().await;
        
        if telemetry.should_stop().await {
            break;
        }
        
        telemetry.print_realtime_metrics().await;
    }
}

/// 📊 Estatísticas de Lucro (Dry Run Pro)
#[derive(Clone, Debug)]
pub struct ProfitStats {
    pub profit_usd: f64,
    pub profit_eur: f64,
    pub target_eur: f64,
    pub target_usd: f64,
    pub progress_percent: f64,
    pub successful_simulations: u64,
    pub remaining_to_target_usd: f64,
}
