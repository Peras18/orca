//! 🔮 Filtro de Kalman para previsão de deriva de preço por pool (sqrt_price_x96)
//! Extensão direta do KalmanGasPredictor (mesmo padrão, já provado no gas)
//! aplicada à variável que realmente causa os reverts IIA: o preço da pool
//! entre o instante de deteção e o instante do eth_call. Estado: [preço, velocidade].
//! Objetivo: dimensionar o input para onde o preço VAI estar (t+latência_estimada),
//! não onde estava na deteção -- ataca a causa raiz do IIA, não latência de rede.

#[derive(Debug, Clone)]
pub struct KalmanPricePredictor {
    x: [f64; 2],
    p: [[f64; 2]; 2],
    q: [[f64; 2]; 2],
    r: f64,
    prediction_errors: Vec<f64>,
    pub last_observed: f64,
    pub last_update_ms: u64,
}

impl KalmanPricePredictor {
    pub fn new(initial_sqrt_price: f64) -> Self {
        Self {
            x: [initial_sqrt_price, 0.0],
            p: [[1.0, 0.0], [0.0, 1.0]],
            q: [[1e-6, 0.0], [0.0, 1e-8]], // preço move-se muito menos por passo que gas
            r: 1e-4,
            prediction_errors: Vec::with_capacity(50),
            last_observed: initial_sqrt_price,
            last_update_ms: 0,
        }
    }

    /// Atualiza com preço observado (normalizado, ex: sqrt_price_x96 as f64 / 2^96)
    /// e timestamp ms da observação. Retorna velocidade estimada (unid/ms).
    pub fn update(&mut self, observed_price: f64, now_ms: u64) -> f64 {
        let dt_ms = if self.last_update_ms == 0 { 100.0 } else { (now_ms.saturating_sub(self.last_update_ms)).max(100) as f64 };
        // CORREÇÃO: dt mínimo de 1ms amplificava ruído em velocidade absurda quando
        // duas observações da mesma pool chegavam na mesma rajada de eventos
        // (comum) -- innovation/dt com dt~0 gerava derivas espúrias que empurravam
        // tudo para o modo mais conservador do sizing, matando candidatos válidos.
        // Normalizar velocidade para "por ms" mantém o filtro estável independente
        // do espaçamento irregular entre eventos (ao contrário do gas, que é por bloco fixo).
        let x_pred = [self.x[0] + self.x[1] * dt_ms, self.x[1]];
        let p_pred = [
            [self.p[0][0] + self.p[1][0]*dt_ms + self.p[0][1]*dt_ms + self.p[1][1]*dt_ms*dt_ms + self.q[0][0], self.p[0][1] + self.p[1][1]*dt_ms + self.q[0][1]],
            [self.p[1][0] + self.p[1][1]*dt_ms + self.q[1][0], self.p[1][1] + self.q[1][1]],
        ];
        let innovation = observed_price - x_pred[0];
        self.prediction_errors.push(innovation.abs());
        if self.prediction_errors.len() > 50 {
            self.prediction_errors.remove(0);
            let avg_err = self.prediction_errors.iter().sum::<f64>() / self.prediction_errors.len() as f64;
            self.r = (avg_err * avg_err).max(1e-6);
        }
        let s = p_pred[0][0] + self.r;
        let k = [p_pred[0][0] / s, p_pred[1][0] / s];
        self.x = [x_pred[0] + k[0]*innovation, x_pred[1] + k[1]*innovation];
        self.p = [
            [(1.0-k[0])*p_pred[0][0], (1.0-k[0])*p_pred[0][1]],
            [-k[1]*p_pred[0][0] + p_pred[1][0], -k[1]*p_pred[0][1] + p_pred[1][1]],
        ];
        self.last_observed = observed_price;
        self.last_update_ms = now_ms;
        self.x[1]
    }

    /// Prevê o preço daqui a `horizon_ms` milissegundos (ex: latência p50 medida ~1900ms)
    pub fn predict_ahead(&self, horizon_ms: f64) -> f64 {
        (self.x[0] + self.x[1] * horizon_ms).max(0.0)
    }

    /// Velocidade estimada (unid/ms) -- usada para filtrar candidatos com
    /// deriva de preço demasiado rápida (pool volátil AGORA, mesmo que o
    /// sizing esteja correto, o risco de o eth_call já não bater certo sobe).
    pub fn velocity(&self) -> f64 {
        self.x[1]
    }

    /// Deriva relativa prevista sobre o horizonte, como fração do preço atual.
    pub fn relative_drift(&self, horizon_ms: f64) -> f64 {
        if self.x[0].abs() < 1e-18 { return 0.0; }
        (self.x[1] * horizon_ms / self.x[0]).abs()
    }
}
