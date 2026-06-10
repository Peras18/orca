//! Topologia de Persistência aplicada a grafos AMM
//!
//! Trata ciclos de arbitragem como features topológicas (H1 — 1-cycles)
//! num complexo simplicial filtrado pelo spread.
//!
//! Conceito: cada ciclo tem um "birth" (spread onde se torna lucrativo)
//! e um "death" (spread onde o mercado o fecha). A diferença birth-death
//! é a PERSISTÊNCIA — ciclos com alta persistência são estruturalmente
//! estáveis e reaparecem repetidamente.
//!
//! Algoritmo:
//! 1. Para cada ciclo conhecido, calcular o spread atual
//! 2. Comparar com o spread histórico (birth/death tracking)
//! 3. Ciclos com persistência >= threshold são priorizados
//! 4. Quando um ciclo "nasce" (spread cruza threshold), sinalizar imediatamente

use std::collections::HashMap;
use alloy::primitives::Address;

/// Identificador único de um ciclo: ordered tuple de pools
type CycleId = Vec<Address>;

/// Feature topológica de um ciclo
#[derive(Debug, Clone)]
pub struct PersistentFeature {
    /// Identificador do ciclo
    pub cycle_id: CycleId,
    /// Spread no momento de "nascimento" (primeira vez lucrativo)
    pub birth_spread: f64,
    /// Spread no momento de "morte" (última vez que foi lucrativo)  
    pub death_spread: f64,
    /// Persistência = death - birth (quanto tempo sobreviveu)
    pub persistence: f64,
    /// Número de vezes que este ciclo renasceu
    pub revival_count: u32,
    /// Bloco do último nascimento
    pub last_birth_block: u64,
    /// Spread médio histórico
    pub avg_spread: f64,
    /// Contagem de observações
    pub observation_count: u32,
    /// Estado atual: true = vivo (lucrativo agora)
    pub alive: bool,
}

impl PersistentFeature {
    fn new(cycle_id: CycleId, birth_spread: f64, block: u64) -> Self {
        Self {
            cycle_id,
            birth_spread,
            death_spread: 0.0,
            persistence: 0.0,
            revival_count: 0,
            last_birth_block: block,
            avg_spread: birth_spread,
            observation_count: 1,
            alive: true,
        }
    }

    /// Atualiza com nova observação de spread
    fn observe(&mut self, spread: f64, block: u64, alive_threshold: f64) {
        let was_alive = self.alive;
        self.alive = spread >= alive_threshold;
        self.observation_count += 1;
        // Running average do spread
        self.avg_spread = (self.avg_spread * (self.observation_count - 1) as f64 + spread)
            / self.observation_count as f64;

        if !was_alive && self.alive {
            // Nascimento: ciclo voltou a ser lucrativo
            self.revival_count += 1;
            self.last_birth_block = block;
            self.birth_spread = spread;
        } else if was_alive && !self.alive {
            // Morte: ciclo deixou de ser lucrativo
            self.death_spread = spread;
            self.persistence = self.birth_spread - self.death_spread;
        }
    }

    /// Score de prioridade: combina persistência, revival rate e spread atual
    pub fn priority_score(&self, current_spread: f64) -> f64 {
        let persistence_score = self.persistence.max(0.0) * 1000.0;
        let revival_score = self.revival_count as f64 * 500.0;
        let spread_score = current_spread * 10000.0;
        persistence_score + revival_score + spread_score
    }
}

/// Sinal de nascimento de um ciclo
#[derive(Debug, Clone)]
pub struct BirthSignal {
    pub cycle_id: CycleId,
    pub spread: f64,
    pub persistence_score: f64,
    pub block: u64,
    /// true = primeira vez, false = revival
    pub is_revival: bool,
}

#[derive(Debug)]
pub struct PersistentTopology {
    features: HashMap<String, PersistentFeature>,
    /// Threshold mínimo de spread para considerar ciclo vivo (0.1% = 0.001)
    alive_threshold: f64,
    /// Persistência mínima para considerar ciclo fiável
    min_persistence: f64,
}

impl PersistentTopology {
    pub fn new() -> Self {
        Self {
            features: HashMap::new(),
            alive_threshold: 0.001, // 0.1% spread mínimo
            min_persistence: 0.0005, // 0.05% persistência mínima
        }
    }

    fn cycle_key(pools: &[Address]) -> String {
        let mut sorted: Vec<String> = pools.iter().map(|a| format!("{:?}", a)).collect();
        sorted.sort();
        sorted.join("|")
    }

    /// Observa o spread atual de um ciclo e retorna sinal se nasceu
    pub fn observe_cycle(
        &mut self,
        pools: &[Address],
        spread: f64,
        block: u64,
    ) -> Option<BirthSignal> {
        let key = Self::cycle_key(pools);
        let threshold = self.alive_threshold;

        if let Some(feature) = self.features.get_mut(&key) {
            let was_alive = feature.alive;
            feature.observe(spread, block, threshold);

            // Nascimento ou revival
            if !was_alive && feature.alive {
                let score = feature.priority_score(spread);
                let is_revival = feature.revival_count > 0;
                return Some(BirthSignal {
                    cycle_id: pools.to_vec(),
                    spread,
                    persistence_score: score,
                    block,
                    is_revival,
                });
            }
        } else if spread >= threshold {
            // Novo ciclo — primeira observação lucrativa
            let cycle_id = pools.to_vec();
            let feature = PersistentFeature::new(cycle_id.clone(), spread, block);
            self.features.insert(key, feature);
            return Some(BirthSignal {
                cycle_id,
                spread,
                persistence_score: spread * 10000.0,
                block,
                is_revival: false,
            });
        } else {
            // Ciclo novo mas não lucrativo — regista para tracking futuro
            let cycle_id = pools.to_vec();
            let mut feature = PersistentFeature::new(cycle_id, 0.0, block);
            feature.alive = false;
            feature.birth_spread = 0.0;
            self.features.insert(key, feature);
        }

        None
    }

    /// Retorna os ciclos mais persistentes ordenados por score
    pub fn top_persistent_cycles(&self, n: usize) -> Vec<&PersistentFeature> {
        let min_p = self.min_persistence;
        let mut features: Vec<&PersistentFeature> = self.features.values()
            .filter(|f| f.persistence >= min_p || f.revival_count > 0)
            .collect();
        features.sort_by(|a, b| {
            let sa = a.priority_score(a.avg_spread);
            let sb = b.priority_score(b.avg_spread);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        features.truncate(n);
        features
    }

    /// Boost de prioridade para um pool baseado nos ciclos persistentes que o contém
    pub fn pool_persistence_boost(&self, pool: Address) -> f64 {
        let pool_str = format!("{:?}", pool);
        self.features.values()
            .filter(|f| f.alive || f.revival_count > 0)
            .filter(|f| f.cycle_id.iter().any(|p| format!("{:?}", p) == pool_str))
            .map(|f| f.priority_score(f.avg_spread))
            .sum()
    }

    pub fn feature_count(&self) -> usize {
        self.features.len()
    }

    pub fn alive_count(&self) -> usize {
        self.features.values().filter(|f| f.alive).count()
    }
}