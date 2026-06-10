//! 🔮 Filtro de Kalman para previsão de gas price bloco a bloco
//! Estado: [gas_price, gas_velocity] (preço e taxa de mudança)
//! Observação: gas_price real do bloco

#[derive(Debug, Clone)]
pub struct KalmanGasPredictor {
    /// Estado estimado: [price, velocity]
    x: [f64; 2],
    /// Covariância do erro de estimativa
    p: [[f64; 2]; 2],
    /// Ruído do processo (incerteza do modelo)
    q: [[f64; 2]; 2],
    /// Ruído de observação (incerteza da medição)
    r: f64,
    /// Histórico de previsões vs real (para auto-tuning)
    prediction_errors: Vec<f64>,
    /// Último gas price real observado
    pub last_observed: f64,
    /// Previsão para o próximo bloco
    pub next_prediction: f64,
}

impl KalmanGasPredictor {
    pub fn new(initial_gas_gwei: f64) -> Self {
        Self {
            x: [initial_gas_gwei, 0.0],
            p: [[1.0, 0.0], [0.0, 1.0]],
            // Q: processo tem alguma variabilidade
            q: [[0.01, 0.0], [0.0, 0.001]],
            // R: observação tem ruído moderado
            r: 0.1,
            prediction_errors: Vec::with_capacity(100),
            last_observed: initial_gas_gwei,
            next_prediction: initial_gas_gwei,
        }
    }

    /// Atualiza com novo gas price observado, retorna previsão para próximo bloco
    pub fn update(&mut self, observed_gas_gwei: f64) -> f64 {
        // ── PREDICT ──
        // Modelo: price(t+1) = price(t) + velocity(t)
        //         velocity(t+1) = velocity(t)  [constante]
        let x_pred = [
            self.x[0] + self.x[1],
            self.x[1],
        ];
        // P_pred = F * P * F^T + Q  (F = [[1,1],[0,1]])
        let p_pred = [
            [
                self.p[0][0] + self.p[1][0] + self.p[0][1] + self.p[1][1] + self.q[0][0],
                self.p[0][1] + self.p[1][1] + self.q[0][1],
            ],
            [
                self.p[1][0] + self.p[1][1] + self.q[1][0],
                self.p[1][1] + self.q[1][1],
            ],
        ];

        // ── UPDATE ──
        // Innovation: y = observed - H*x_pred  (H = [1, 0])
        let innovation = observed_gas_gwei - x_pred[0];

        // Erro de previsão para auto-tuning
        self.prediction_errors.push(innovation.abs());
        if self.prediction_errors.len() > 50 {
            self.prediction_errors.remove(0);
            // Auto-tune R baseado no erro médio recente
            let avg_err = self.prediction_errors.iter().sum::<f64>()
                / self.prediction_errors.len() as f64;
            self.r = (avg_err * avg_err).max(0.01);
        }

        // S = H*P_pred*H^T + R
        let s = p_pred[0][0] + self.r;

        // Kalman gain: K = P_pred * H^T / S
        let k = [p_pred[0][0] / s, p_pred[1][0] / s];

        // Update estado
        self.x = [
            x_pred[0] + k[0] * innovation,
            x_pred[1] + k[1] * innovation,
        ];

        // Update covariância: P = (I - K*H) * P_pred
        self.p = [
            [(1.0 - k[0]) * p_pred[0][0], (1.0 - k[0]) * p_pred[0][1]],
            [-k[1] * p_pred[0][0] + p_pred[1][0], -k[1] * p_pred[0][1] + p_pred[1][1]],
        ];

        self.last_observed = observed_gas_gwei;
        self.next_prediction = self.x[0].max(0.001); // mínimo 0.001 gwei
        self.next_prediction
    }

    /// Retorna previsão com margem de segurança (para garantir inclusão no bloco)
    pub fn safe_gas_price_gwei(&self) -> f64 {
        // Previsão + 1 sigma de incerteza
        let uncertainty = self.p[0][0].sqrt();
        (self.next_prediction + uncertainty * 1.5).max(0.001)
    }

    /// Accuracy das últimas previsões (0-1, 1 = perfeito)
    pub fn accuracy(&self) -> f64 {
        if self.prediction_errors.is_empty() { return 1.0; }
        let avg_err = self.prediction_errors.iter().sum::<f64>()
            / self.prediction_errors.len() as f64;
        (1.0 - (avg_err / self.last_observed.max(0.001))).max(0.0)
    }
}
