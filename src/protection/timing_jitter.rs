//! ⏱️ TimingJitter — Adiciona atraso aleatório para evitar padrões previsíveis
//!
//! Submissão de bundles em intervalos fixos é um sinal óbvio de bot automatizado.
//! Este módulo distribui submissões ao longo de uma janela de tempo aleatória.

use rand::Rng;
use tokio::time::{sleep, Duration};
use tracing::debug;

/// Configuração de jitter de timing
pub struct TimingJitterConfig {
    /// Atraso mínimo base em ms
    pub min_delay_ms: u64,
    /// Atraso máximo aleatório em ms
    pub max_jitter_ms: u64,
    /// Probabilidade de saltar uma oportunidade (para parecer humano)
    pub skip_chance: f64,
    /// Variabilidade da frequência (simula "pensamento" humano)
    pub think_time_ms: u64,
}

impl Default for TimingJitterConfig {
    fn default() -> Self {
        Self {
            min_delay_ms: 50,      // 50ms mínimo (ultra-low latency)
            max_jitter_ms: 300,    // até +300ms de jitter
            skip_chance: 0.05,     // 5% de chance de ignorar oportunidade
            think_time_ms: 500,    // 500ms de "think time" ocasional
        }
    }
}

/// Distribui atrasos aleatórios nas submissões
pub struct TimingJitter {
    config: TimingJitterConfig,
}

impl TimingJitter {
    pub fn new(config: TimingJitterConfig) -> Self {
        Self { config }
    }

    /// Adiciona atraso aleatório antes de uma ação
    pub async fn jitter(&self) {
        let mut rng = rand::thread_rng();

        // Atraso base + jitter aleatório
        let jitter = rng.gen_range(0..=self.config.max_jitter_ms);
        let total_delay = self.config.min_delay_ms + jitter;

        // Ocasionamente simular "think time" (como um humano analisando)
        if rng.gen::<f64>() < 0.10 {
            let think = rng.gen_range(self.config.think_time_ms..=self.config.think_time_ms * 3);
            debug!("⏱️ TimingJitter: think-time de {}ms", think);
            sleep(Duration::from_millis(think)).await;
            return;
        }

        debug!("⏱️ TimingJitter: delay de {}ms", total_delay);
        sleep(Duration::from_millis(total_delay)).await;
    }

    /// Decide se deve executar ou ignorar (para quebrar padrões)
    pub fn should_execute(&self) -> bool {
        let mut rng = rand::thread_rng();
        let should = rng.gen::<f64>() >= self.config.skip_chance;
        if !should {
            debug!("⏱️ TimingJitter: oportunidade ignorada para evitar padrão");
        }
        should
    }

    /// Calcula deadline de bloco com margem de segurança aleatória
    pub fn randomized_deadline(&self, current_block: u64) -> u64 {
        let mut rng = rand::thread_rng();
        let margin = rng.gen_range(2..=5); // 2-5 blocos de margem
        current_block + margin
    }
}
