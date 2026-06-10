//! ⚡ Flash Loan Optimizer — Newton-Raphson para encontrar input ótimo
//! Maximiza: profit(x) = output(x) - x - gas_cost
//! Derivada numérica: f'(x) ≈ (f(x+h) - f(x-h)) / 2h

#[derive(Debug, Clone)]
pub struct FlashLoanOptimizer {
    /// Tolerância de convergência (wei)
    pub tolerance: f64,
    /// Máximo de iterações
    pub max_iter: usize,
    /// Histórico de inputs ótimos por pool-pair
    optimal_cache: std::collections::HashMap<String, u128>,
}

impl FlashLoanOptimizer {
    pub fn new() -> Self {
        Self {
            tolerance: 1e10, // 0.00001 ETH
            max_iter: 20,
            optimal_cache: std::collections::HashMap::new(),
        }
    }

    /// Encontra o input ótimo via Newton-Raphson
    /// f(x) = profit(x) = simulate_path(x) - x - gas_cost
    /// Maximizar f(x) ↔ encontrar f'(x) = 0
    pub fn optimize<F>(
        &mut self,
        path_key: &str,
        simulate: F,         // função: input_wei → output_wei
        gas_cost_wei: u128,
        min_input: u128,
        max_input: u128,
    ) -> Option<u128>
    where
        F: Fn(u128) -> u128,
    {
        // Ponto inicial: cache ou meio do intervalo
        let x0 = self.optimal_cache
            .get(path_key)
            .copied()
            .unwrap_or((min_input + max_input) / 2);

        let mut x = x0 as f64;
        let min_f = min_input as f64;
        let max_f = max_input as f64;

        for _iter in 0..self.max_iter {
            let h = x * 0.01; // 1% de step
            if h < 1e6 { break; }

            let x_minus = ((x - h) as u128).max(min_input);
            let x_plus = ((x + h) as u128).min(max_input);

            let profit = |inp: u128| -> f64 {
                let out = simulate(inp);
                if out <= inp { return -(gas_cost_wei as f64); }
                (out - inp) as f64 - gas_cost_wei as f64
            };

            let f_minus = profit(x_minus);
            let f_center = profit(x as u128);
            let f_plus = profit(x_plus);

            // Primeira derivada (gradiente)
            let f_prime = (f_plus - f_minus) / (2.0 * h);

            // Segunda derivada (curvatura)
            let f_double_prime = (f_plus - 2.0 * f_center + f_minus) / (h * h);

            // Newton-Raphson: x_new = x - f'(x) / f''(x)
            if f_double_prime.abs() < 1e-20 { break; }
            let x_new = x - f_prime / f_double_prime;

            // Clamp ao intervalo válido
            let x_new = x_new.max(min_f).min(max_f);

            // Convergência
            if (x_new - x).abs() < self.tolerance { 
                x = x_new;
                break;
            }
            x = x_new;
        }

        let optimal = (x as u128).max(min_input).min(max_input);
        
        // Só retorna se lucrativo
        let out = simulate(optimal);
        if out <= optimal + gas_cost_wei {
            return None;
        }

        // Cache para próxima vez
        self.optimal_cache.insert(path_key.to_string(), optimal);
        Some(optimal)
    }

    /// Retorna o profit estimado para o input ótimo
    pub fn optimal_profit<F>(
        &mut self,
        path_key: &str,
        simulate: F,
        gas_cost_wei: u128,
        min_input: u128,
        max_input: u128,
    ) -> Option<(u128, u128)> // (optimal_input, net_profit)
    where
        F: Fn(u128) -> u128 + Clone,
    {
        let optimal = self.optimize(path_key, simulate.clone(), gas_cost_wei, min_input, max_input)?;
        let output = simulate(optimal);
        if output <= optimal { return None; }
        let gross = output - optimal;
        if gross <= gas_cost_wei { return None; }
        Some((optimal, gross - gas_cost_wei))
    }
}
