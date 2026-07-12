//! 📊 Modelo Bayesiano de probabilidade de sucesso por segmento (dex_types+hop_count).
//! Prior Beta(1,1) (uniforme, sem viés) por segmento; atualiza com cada resultado
//! real (sucesso/revert). Com poucos dados, o prior mantém otimismo (não bloqueia
//! segmentos novos); só filtra quando há amostra suficiente E taxa de sucesso baixa
//! com confiança estatística real -- dormente até haver variância real nos dados.

use dashmap::DashMap;

#[derive(Debug)]
pub struct BayesianSuccessModel {
    // key -> (alpha, beta) da distribuição Beta
    segments: DashMap<String, (f64, f64)>,
}

impl BayesianSuccessModel {
    pub fn new() -> Self {
        Self { segments: DashMap::new() }
    }

    pub fn record(&self, key: &str, success: bool) {
        let mut entry = self.segments.entry(key.to_string()).or_insert((1.0, 1.0));
        if success {
            entry.0 += 1.0;
        } else {
            entry.1 += 1.0;
        }
    }

    /// Média posterior + nº de amostras reais (excluindo o prior).
    pub fn estimate(&self, key: &str) -> (f64, f64) {
        match self.segments.get(key) {
            Some(entry) => {
                let (a, b) = *entry;
                let mean = a / (a + b);
                let n_real = (a + b) - 2.0; // remove o prior (1,1)
                (mean, n_real.max(0.0))
            }
            None => (0.5, 0.0), // sem dados -- prior neutro, não bloqueia
        }
    }

    /// Verdadeiro se este segmento deve ser evitado (amostra suficiente E
    /// taxa de sucesso posterior abaixo do limiar).
    pub fn should_avoid(&self, key: &str, min_samples: f64, threshold: f64) -> bool {
        let (mean, n) = self.estimate(key);
        n >= min_samples && mean < threshold
    }
}
