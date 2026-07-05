//! NEWTON-RAPHSON JACOBIAN SOLVER - Arbitragem Triangular Avançada
//!
//! Resolve sistema de 3 equações simultâneas para 3 pools:
//! F1(x,y,z) = lucro_pool1(x,y) - taxa_flashloan = 0
//! F2(x,y,z) = lucro_pool2(y,z) - taxa_flashloan = 0  
//! F3(x,y,z) = lucro_pool3(z,x) - taxa_flashloan = 0
//!
//! Convergência: Lucro líquido > $20.00 (ajustável)

use alloy::primitives::{Address, U256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug, warn};

use crate::types::PoolReserves;
// use crate::types::DexType; // Not used currently
use crate::strategist::multi_hop_engine::FlashloanProvider;

/// 🎯 FLASHLOAN FEE CONFIGURATION
pub const FLASHLOAN_FEE_AAVE_BPS: u32 = 9;      // 0.09%
pub const FLASHLOAN_FEE_UNISWAP_BPS: u32 = 30; // 0.3%
pub const FLASHLOAN_FEE_BALANCER_BPS: u32 = 0;  // 0%

/// 💰 MINIMUM PROFIT THRESHOLD (ajustável)
pub const MIN_PROFIT_LIQUIDO_USD: f64 = 20.00;
pub const TARGET_PROFIT_DAILY_EUR: f64 = 3000.0;

/// 🔢 JACOBIAN SOLVER PARA 3 POOLS
pub struct NewtonJacobianSolver {
    /// Reservas das pools
    reserves: Arc<RwLock<HashMap<Address, PoolReserves>>>,
    
    /// Flashloan provider preferido
    flashloan_provider: FlashloanProvider,
    
    /// Flashloan fee em basis points
    flashloan_fee_bps: u32,
    
    /// Preço do ETH (USD)
    eth_price_usd: f64,
    
    /// Tolerância de convergência
    tolerance: f64,
    
    /// Máximo de iterações
    max_iterations: u32,
}

/// 📊 Sistema de 3 Equações (Arbitragem Triangular)
#[derive(Clone, Debug)]
pub struct TriangularSystem {
    /// Pool A (ex: WETH/USDC)
    pub pool_a: Address,
    /// Pool B (ex: USDC/AERO)  
    pub pool_b: Address,
    /// Pool C (ex: AERO/WETH)
    pub pool_c: Address,
    
    /// Tokens: [WETH, USDC, AERO, WETH]
    pub tokens: [Address; 4],
    
    /// DEXs das pools
    pub dex_fees: [u32; 3], // basis points
}

/// 💰 Resultado da Solução
#[derive(Clone, Debug)]
pub struct SolverResult {
    /// Quantidade ótima de flashloan (ETH)
    pub optimal_flashloan_eth: f64,
    /// Quantidade em U256
    pub optimal_amount_wei: U256,
    /// Lucro bruto (USD)
    pub gross_profit_usd: f64,
    /// Taxa de flashloan (USD)
    pub flashloan_fee_usd: f64,
    /// Lucro LÍQUIDO (USD)
    pub net_profit_usd: f64,
    /// Convergiu?
    pub converged: bool,
    /// Número de iterações
    pub iterations: u32,
    /// Matriz Jacobiana final
    pub final_jacobian: [[f64; 3]; 3],
    /// Resíduos finais
    pub final_residuals: [f64; 3],
    /// Provider usado
    pub provider: FlashloanProvider,
}

/// 🧮 MATRIZ JACOBIANA 3x3
/// 
/// J = [∂F1/∂x  ∂F1/∂y  ∂F1/∂z]
///     [∂F2/∂x  ∂F2/∂y  ∂F2/∂z]
///     [∂F3/∂x  ∂F3/∂y  ∂F3/∂z]
///
/// Onde:
/// - x: amount_in pool A
/// - y: amount_in pool B  
/// - z: amount_in pool C
pub type Jacobian3x3 = [[f64; 3]; 3];

/// 📊 Estado do Sistema
#[derive(Clone, Debug)]
struct SystemState {
    /// Quantidades atuais [x, y, z]
    amounts: [f64; 3],
    /// Resíduos [F1, F2, F3]
    residuals: [f64; 3],
    /// Jacobiana
    jacobian: Jacobian3x3,
}

impl NewtonJacobianSolver {
    pub fn new(provider: FlashloanProvider, eth_price_usd: f64) -> Self {
        let fee_bps = match provider {
            FlashloanProvider::AaveV3 => FLASHLOAN_FEE_AAVE_BPS,
            FlashloanProvider::BalancerV2 => FLASHLOAN_FEE_BALANCER_BPS,
            FlashloanProvider::UniswapV3 => FLASHLOAN_FEE_UNISWAP_BPS,
        };
        
        Self {
            reserves: Arc::new(RwLock::new(HashMap::new())),
            flashloan_provider: provider,
            flashloan_fee_bps: fee_bps,
            eth_price_usd,
            tolerance: 0.001, // 0.1% precision
            max_iterations: 50,
        }
    }
    
    /// 🎯 SOLVE MAX PROFIT: Encontra a quantidade ótima de flashloan para lucro MÁXIMO
    /// 
    /// Resolve d(Lucro)/dx = 0
    pub async fn solve_max_profit(
        &self,
        system: &TriangularSystem,
    ) -> Option<SolverResult> {
        info!("🔢🔢🔢 [JACOBIAN] Iniciando solver de MAXIMIZAÇÃO");
        
        let mut x = 1.0f64; // Chute inicial: 1 ETH
        let mut prev_x = 0.0f64;
        
        for iteration in 0..self.max_iterations {
            if f64::abs(x - prev_x) < self.tolerance {
                debug!("[JACOBIAN] Convergiu para lucro máximo: {:.4} ETH", x);
                return self.build_result_from_x(system, x, iteration, true).await;
            }
            
            prev_x = x;
            
            // Calcular derivada primeira e segunda do lucro
            // P(x) = output(x) - x*(1+fee)
            // P'(x) = output'(x) - (1+fee)
            // P''(x) = output''(x)
            
            let (p_prime, p_double_prime) = self.calculate_profit_derivatives(system, x).await;
            
            if p_double_prime.abs() < 1e-12 {
                break;
            }
            
            // Newton step: x = x - P'(x)/P''(x)
            x -= p_prime / p_double_prime;
            
            // Constraints
            if x < 0.001 { x = 0.001; }
            if x > 40.0 { x = 40.0; } // User limit: ~100k€ @ $2500/ETH
        }
        
        self.build_result_from_x(system, x, self.max_iterations, false).await
    }

    /// 🧮 Calcula derivadas do lucro via diferenças finitas centrais
    async fn calculate_profit_derivatives(
        &self,
        system: &TriangularSystem,
        x: f64,
    ) -> (f64, f64) {
        let h = 0.0001;
        
        let p_plus = self.calculate_net_profit(system, x + h).await;
        let p_minus = self.calculate_net_profit(system, x - h).await;
        let p_center = self.calculate_net_profit(system, x).await;
        
        let p_prime = (p_plus - p_minus) / (2.0 * h);
        let p_double_prime = (p_plus - 2.0 * p_center + p_minus) / (h * h);
        
        (p_prime, p_double_prime)
    }

    /// 💰 Calcula lucro líquido para um dado input x
    async fn calculate_net_profit(
        &self,
        system: &TriangularSystem,
        x: f64,
    ) -> f64 {
        let reserves = self.reserves.read().await;
        
        let out_a = self.simulate_swap(reserves.get(&system.pool_a).cloned().unwrap_or_default(), x).await;
        let out_b = self.simulate_swap(reserves.get(&system.pool_b).cloned().unwrap_or_default(), out_a).await;
        let out_c = self.simulate_swap(reserves.get(&system.pool_c).cloned().unwrap_or_default(), out_b).await;
        
        let fee_factor = 1.0 + (self.flashloan_fee_bps as f64 / 10000.0);
        out_c - x * fee_factor
    }

    /// 🏗️ Constrói resultado a partir de um input x
    async fn build_result_from_x(
        &self,
        system: &TriangularSystem,
        x: f64,
        iterations: u32,
        converged: bool,
    ) -> Option<SolverResult> {
        let reserves = self.reserves.read().await;
        
        let out_a = self.simulate_swap(reserves.get(&system.pool_a).cloned().unwrap_or_default(), x).await;
        let out_b = self.simulate_swap(reserves.get(&system.pool_b).cloned().unwrap_or_default(), out_a).await;
        let out_c = self.simulate_swap(reserves.get(&system.pool_c).cloned().unwrap_or_default(), out_b).await;
        
        let fee_factor = 1.0 + (self.flashloan_fee_bps as f64 / 10000.0);
        let flashloan_fee_eth = x * (self.flashloan_fee_bps as f64 / 10000.0);
        let net_profit_eth = out_c - x * fee_factor;
        
        if net_profit_eth <= 0.0 {
            return None;
        }

        Some(SolverResult {
            optimal_flashloan_eth: x,
            optimal_amount_wei: U256::from((x * 1e18) as u128),
            gross_profit_usd: (out_c - x) * self.eth_price_usd,
            flashloan_fee_usd: flashloan_fee_eth * self.eth_price_usd,
            net_profit_usd: net_profit_eth * self.eth_price_usd,
            converged,
            iterations,
            final_jacobian: [[0.0; 3]; 3], // Simplificado para este solver
            final_residuals: [0.0; 3],
            provider: self.flashloan_provider.clone(),
        })
    }

    /// 🎯 SOLVE CROSS-DEX: Otimização multi-variável para rotas complexas (N pools)
    pub async fn solve_cross_dex_arbitrage(
        &self,
        pools: Vec<Address>,
        tokens: Vec<Address>,
    ) -> Option<SolverResult> {
        info!("🔢 [JACOBIAN-V2] Resolvendo Cross-DEX para {} hops", pools.len());
        
        let mut x = 1.0;
        let h = 0.0001;
        
        for _iteration in 0..self.max_iterations {
            let p_center = self.simulate_multi_hop_path(&pools, &tokens, x).await;
            let p_plus = self.simulate_multi_hop_path(&pools, &tokens, x + h).await;
            let p_minus = self.simulate_multi_hop_path(&pools, &tokens, x - h).await;

            let p_prime = (p_plus - p_minus) / (2.0 * h);
            let p_double_prime = (p_plus - 2.0 * p_center + p_minus) / (h * h);

            if p_double_prime.abs() < 1e-12 { break; }

            let delta = p_prime / p_double_prime;
            x -= delta;

            if x < 0.001 { x = 0.001; }
            if x > 100.0 { x = 100.0; } // Escala para $250k+

            if delta.abs() < 0.0001 { break; }
        }
        
        self.build_result_from_x_v2(&pools, &tokens, x).await
    }

    async fn simulate_multi_hop_path(&self, pools: &[Address], _tokens: &[Address], amount_in: f64) -> f64 {
        let reserves = self.reserves.read().await;
        let mut current_amount = amount_in;
        
        for i in 0..pools.len() {
            let pool = reserves.get(&pools[i]).cloned().unwrap_or_default();
            current_amount = self.simulate_swap(pool, current_amount).await;
        }
        
        let fee_factor = 1.0 + (self.flashloan_fee_bps as f64 / 10000.0);
        current_amount - amount_in * fee_factor
    }

    async fn build_result_from_x_v2(&self, pools: &[Address], tokens: &[Address], x: f64) -> Option<SolverResult> {
        let net_profit_eth = self.simulate_multi_hop_path(pools, tokens, x).await;
        
        if net_profit_eth <= 0.0 { return None; }

        Some(SolverResult {
            optimal_flashloan_eth: x,
            optimal_amount_wei: U256::from((x * 1e18) as u128),
            gross_profit_usd: (net_profit_eth + x * (self.flashloan_fee_bps as f64 / 10000.0)) * self.eth_price_usd,
            flashloan_fee_usd: (x * (self.flashloan_fee_bps as f64 / 10000.0)) * self.eth_price_usd,
            net_profit_usd: net_profit_eth * self.eth_price_usd,
            converged: true,
            iterations: 0,
            final_jacobian: [[0.0; 3]; 3],
            final_residuals: [0.0; 3],
            provider: self.flashloan_provider.clone(),
        })
    }

    async fn simulate_swap(&self, pool: PoolReserves, amount_in: f64) -> f64 {
        if amount_in <= 0.0 { return 0.0; }
        
        let r_in = pool.reserve0.try_into().unwrap_or(u128::MAX) as f64 / 1e18; // Assumindo ETH em pool de 18 decimais
        let r_out = pool.reserve1.try_into().unwrap_or(u128::MAX) as f64 / 1e18;
        let fee = pool.fee as f64 / 1_000_000.0;
        
        // Uniswap V2 constant product: (r_in + amount_in * (1-fee)) * (r_out - amount_out) = r_in * r_out
        let amount_in_with_fee = amount_in * (1.0 - fee);
        
        // 🚨 PRICE IMPACT AVALIAÇÃO (HFT ELITE)
        // Se o amount_in for > 5% da reserva, o slippage real em mainnet 
        // pode ser maior devido a JIT Liquidity ou MEV bots concorrentes.
        let impact_ratio = amount_in / r_in;
        let slippage_safety_factor = if impact_ratio > 0.05 {
            // Penalização extra para transações gigantes ($250k+)
            1.0 - (impact_ratio * 0.1) // Reduz output em até 10% adicional se for muito grande
        } else {
            1.0
        };

        let amount_out = (amount_in_with_fee * r_out) / (r_in + amount_in_with_fee);
        amount_out * slippage_safety_factor
    }

    /// 🎯 SOLVE: Encontra solução ótima para arbitragem triangular
    /// 
    /// Sistema de equações:
    /// F1(x,y) = output_A(x) - y = 0
    /// F2(y,z) = output_B(y) - z = 0
    /// F3(z,x) = output_C(z) - x*(1+fee) - profit = 0
    pub async fn solve_triangular_arbitrage(
        &self,
        system: &TriangularSystem,
        target_profit_usd: f64,
    ) -> Option<SolverResult> {
        info!("🔢🔢🔢 [JACOBIAN] Iniciando solver para 3 pools");
        info!("    Flashloan Fee: {} bps ({}%)", 
            self.flashloan_fee_bps, self.flashloan_fee_bps as f64 / 100.0);
        
        // Chute inicial: 1 ETH para cada pool
        let mut state = SystemState {
            amounts: [1.0, 1.0, 1.0], // [x, y, z] em ETH
            residuals: [0.0; 3],
            jacobian: [[0.0; 3]; 3],
        };
        
        // Iterações de Newton-Raphson
        for iteration in 0..self.max_iterations {
            // 1. Calcular resíduos F(x,y,z)
            state.residuals = self.calculate_residuals(system, &state.amounts, target_profit_usd).await;
            
            // 2. Verificar convergência
            let max_residual = state.residuals.iter().map(|r| r.abs()).fold(0.0, f64::max);
            
            debug!("[JACOBIAN] Iter {}: max_residual = {:.6}", iteration, max_residual);
            
            if max_residual < self.tolerance {
                // Convergiu! Calcular resultado final
                return self.build_result(system, &state, iteration, true).await;
            }
            
            // 3. Calcular Jacobiana J(x,y,z)
            state.jacobian = self.calculate_jacobian(system, &state.amounts).await;
            
            // 4. Resolver sistema linear: J * Δ = -F
            let delta = self.solve_linear_system(&state.jacobian, &state.residuals);
            
            // 5. Atualizar: x_{n+1} = x_n + Δ
            for i in 0..3 {
                state.amounts[i] -= delta[i]; // Note o sinal negativo (Newton-Raphson)
                
                // Garantir positividade
                if state.amounts[i] < 0.001 {
                    state.amounts[i] = 0.001; // Mínimo 0.001 ETH
                }
            }
            
            // 6. Verificar limites de flashloan
            let total_flashloan = state.amounts[0]; // Pool A inicia com flashloan
            if total_flashloan > 100.0 { // Max 100 ETH ($250k)
                state.amounts[0] = 100.0;
            }
        }
        
        // Não convergiu no máximo de iterações
        warn!("[JACOBIAN] Não convergiu em {} iterações", self.max_iterations);
        self.build_result(system, &state, self.max_iterations, false).await
    }
    
    /// 📊 Calcula resíduos F(x,y,z) para o sistema
    async fn calculate_residuals(
        &self,
        system: &TriangularSystem,
        amounts: &[f64; 3],
        target_profit: f64,
    ) -> [f64; 3] {
        let reserves = self.reserves.read().await;
        
        // Pool A: x (WETH) -> output_A (USDC)
        let pool_a = reserves.get(&system.pool_a).cloned().unwrap_or_default();
        let output_a = self.simulate_swap(pool_a, amounts[0]).await;
        
        // Pool B: y (USDC) -> output_B (AERO)
        let pool_b = reserves.get(&system.pool_b).cloned().unwrap_or_default();
        let output_b = self.simulate_swap(pool_b, amounts[1]).await;
        
        // Pool C: z (AERO) -> output_C (WETH)
        let pool_c = reserves.get(&system.pool_c).cloned().unwrap_or_default();
        let output_c = self.simulate_swap(pool_c, amounts[2]).await;
        drop(reserves);
        
        // Resíduos:
        // F1 = output_A - y (deve ser igual, output_A é input de B)
        // F2 = output_B - z (deve ser igual, output_B é input de C)
        // F3 = output_C - x*(1+fee) - target_profit
        
        let fee_factor = 1.0 + (self.flashloan_fee_bps as f64 / 10000.0);
        let profit_eth = target_profit / self.eth_price_usd;
        
        [
            output_a - amounts[1],                   // F1: output_A deve = y
            output_b - amounts[2],                   // F2: output_B deve = z
            output_c - amounts[0] * fee_factor - profit_eth, // F3: lucro deve = target
        ]
    }
    
    /// 🧮 Calcula Jacobiana por diferenças finitas
    async fn calculate_jacobian(
        &self,
        system: &TriangularSystem,
        amounts: &[f64; 3],
    ) -> Jacobian3x3 {
        let h = 0.001; // Passo para diferenças finitas
        let target_profit = 0.0; // Não importa para Jacobiana
        
        let mut jacobian = [[0.0; 3]; 3];
        
        // Calcular resíduos base
        let f_base = self.calculate_residuals(system, amounts, target_profit).await;
        
        // Coluna j (variável j)
        for j in 0..3 {
            let mut amounts_perturbed = *amounts;
            amounts_perturbed[j] += h;
            
            let f_perturbed = self.calculate_residuals(system, &amounts_perturbed, target_profit).await;
            
            // Linha i (equação i)
            for i in 0..3 {
                jacobian[i][j] = (f_perturbed[i] - f_base[i]) / h;
            }
        }
        
        jacobian
    }
    
    /// 🧮 Resolve sistema linear 3x3: J * x = -F
    /// Usa regra de Cramer para matriz 3x3
    fn solve_linear_system(&self, jacobian: &Jacobian3x3, residuals: &[f64; 3]) -> [f64; 3] {
        let det_j = self.determinant_3x3(jacobian);
        
        if det_j.abs() < 1e-10 {
            // Matriz singular, usar passo pequeno
            return [0.01, 0.01, 0.01];
        }
        
        // Criar matrizes para Cramer
        let mut m_x = *jacobian;
        let mut m_y = *jacobian;
        let mut m_z = *jacobian;
        
        // Substituir colunas por -residuals
        for i in 0..3 {
            m_x[i][0] = -residuals[i];
            m_y[i][1] = -residuals[i];
            m_z[i][2] = -residuals[i];
        }
        
        let det_x = self.determinant_3x3(&m_x);
        let det_y = self.determinant_3x3(&m_y);
        let det_z = self.determinant_3x3(&m_z);
        
        [
            det_x / det_j,
            det_y / det_j,
            det_z / det_j,
        ]
    }
    
    /// 📐 Determinante de matriz 3x3
    fn determinant_3x3(&self, m: &[[f64; 3]; 3]) -> f64 {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }
    
    /// 📊 Constrói resultado final
    async fn build_result(
        &self,
        system: &TriangularSystem,
        state: &SystemState,
        iterations: u32,
        converged: bool,
    ) -> Option<SolverResult> {
        let flashloan_eth = state.amounts[0];
        let flashloan_usd = flashloan_eth * self.eth_price_usd;
        
        // Calcular outputs finais
        let reserves = self.reserves.read().await;
        let pool_c = reserves.get(&system.pool_c).cloned()?;
        drop(reserves);
        
        let final_output = self.simulate_swap(pool_c, state.amounts[2]).await;
        let gross_profit_eth = final_output - flashloan_eth;
        let gross_profit_usd = gross_profit_eth * self.eth_price_usd;
        
        // Taxa de flashloan
        let fee_factor = self.flashloan_fee_bps as f64 / 10000.0;
        let flashloan_fee_eth = flashloan_eth * fee_factor;
        let flashloan_fee_usd = flashloan_fee_eth * self.eth_price_usd;
        
        // Lucro LÍQUIDO
        let net_profit_usd = gross_profit_usd - flashloan_fee_usd;
        
        // Verificar threshold mínimo
        if converged && net_profit_usd < MIN_PROFIT_LIQUIDO_USD {
            warn!("[JACOBIAN] Lucro líquido ${:.2} < mínimo ${:.2}", 
                net_profit_usd, MIN_PROFIT_LIQUIDO_USD);
            return None;
        }
        
        let result = SolverResult {
            optimal_flashloan_eth: flashloan_eth,
            optimal_amount_wei: U256::from((flashloan_eth * 1e18) as u128),
            gross_profit_usd,
            flashloan_fee_usd,
            net_profit_usd,
            converged,
            iterations,
            final_jacobian: state.jacobian,
            final_residuals: state.residuals,
            provider: self.flashloan_provider.clone(),
        };
        
        info!("✅✅✅ [JACOBIAN] Solução encontrada!");
        info!("    Flashloan: {:.2} ETH (${:.0})", flashloan_eth, flashloan_usd);
        info!("    Lucro Bruto: ${:.2}", gross_profit_usd);
        info!("    Taxa Flash: ${:.2} ({}%)", flashloan_fee_usd, fee_factor * 100.0);
        info!("    Lucro LÍQUIDO: ${:.2} 🎯", net_profit_usd);
        info!("    Iterações: {} | Convergiu: {}", iterations, converged);
        
        Some(result)
    }
    
    /// 🔄 Atualiza reservas de uma pool
    pub async fn update_reserves(&self, pool: Address, reserve0: U256, reserve1: U256) {
        let mut reserves = self.reserves.write().await;
        if let Some(r) = reserves.get_mut(&pool) {
            r.reserve0 = reserve0;
            r.reserve1 = reserve1;
        }
    }
    
    /// 🎯 Verifica se vale a pena executar (lucro > $20)
    pub fn should_execute(&self, result: &SolverResult) -> bool {
        result.converged && result.net_profit_usd >= MIN_PROFIT_LIQUIDO_USD
    }
    
    /// 💰 Calcula quantidade de flashloan ótima para 1 pool (simplificado)
    pub fn calculate_optimal_flashloan_single(
        &self,
        reserve_in: f64,
        reserve_out: f64,
        pool_fee_bps: u32,
    ) -> f64 {
        // Fórmula analítica para ótimo em uma pool:
        // optimal = sqrt(reserve_in * reserve_out * fee / (1 - fee)) - reserve_in
        
        let fee = pool_fee_bps as f64 / 10000.0;
        let flashloan_fee = self.flashloan_fee_bps as f64 / 10000.0;
        let total_fee = fee + flashloan_fee;
        
        if total_fee >= 1.0 {
            return 0.0; // Não há lucro possível
        }
        
        let sqrt_term = (reserve_in * reserve_out * total_fee / (1.0 - total_fee)).sqrt();
        let optimal = sqrt_term - reserve_in;
        
        optimal.max(0.0)
    }
}

/// 📊 Builder para sistema triangular
pub struct TriangularSystemBuilder {
    pools: Vec<(Address, Address, Address, u32)>, // (pool, token0, token1, fee)
}

impl TriangularSystemBuilder {
    pub fn new() -> Self {
        Self { pools: Vec::new() }
    }
    
    pub fn add_pool(mut self, pool: Address, token0: Address, token1: Address, fee_bps: u32) -> Self {
        self.pools.push((pool, token0, token1, fee_bps));
        self
    }
    
    /// Tenta construir sistema triangular a partir das pools adicionadas
    pub fn build(self) -> Option<TriangularSystem> {
        if self.pools.len() < 3 {
            return None;
        }
        
        // Procurar ciclo: token0 -> token1 -> token2 -> token0
        for i in 0..self.pools.len() {
            for j in 0..self.pools.len() {
                for k in 0..self.pools.len() {
                    if i == j || j == k || i == k {
                        continue;
                    }
                    
                    let (pool_a, t0_a, t1_a, fee_a) = &self.pools[i];
                    let (pool_b, t0_b, t1_b, fee_b) = &self.pools[j];
                    let (pool_c, t0_c, t1_c, fee_c) = &self.pools[k];
                    
                    // Verificar se forma ciclo: t0_a -> t1_a == t0_b -> t1_b == t0_c -> t1_c == t0_a
                    if t1_a == t0_b && t1_b == t0_c && t1_c == t0_a {
                        return Some(TriangularSystem {
                            pool_a: *pool_a,
                            pool_b: *pool_b,
                            pool_c: *pool_c,
                            tokens: [*t0_a, *t1_a, *t1_b, *t1_c],
                            dex_fees: [*fee_a, *fee_b, *fee_c],
                        });
                    }
                }
            }
        }
        
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_determinant_3x3() {
        let solver = NewtonJacobianSolver::new(FlashloanProvider::AaveV3, 2500.0);
        
        // Matriz identidade
        let identity = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        assert!((solver.determinant_3x3(&identity) - 1.0).abs() < 1e-10);
        
        // Matriz diagonal
        let diagonal = [
            [2.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 4.0],
        ];
        assert!((solver.determinant_3x3(&diagonal) - 24.0).abs() < 1e-10);
    }
    
    #[test]
    fn test_optimal_flashloan_calculation() {
        let solver = NewtonJacobianSolver::new(FlashloanProvider::AaveV3, 2500.0);
        
        // Pool com 1000 ETH / 10000 USDC, fee 0.3%
        let reserve_in = 1000.0;
        let reserve_out = 10000.0;
        let pool_fee = 30; // 0.3%
        
        let optimal = solver.calculate_optimal_flashloan_single(reserve_in, reserve_out, pool_fee);
        
        assert!(optimal > 0.0);
        assert!(optimal < 1000.0); // Deve ser menos que reserva
    }
}
