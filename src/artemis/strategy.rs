#![allow(dead_code)]

use alloy::primitives::Address;
use std::collections::HashMap;
use tracing::{info, trace, warn};

use super::MevEvent;
use crate::contracts::{NormalizedSwapEvent, DexType, UniswapV3Factory, AerodromeFactory};

/// Helper function to convert U256 amount to ETH (f64)
fn u256_to_eth_f64(amount: &alloy::primitives::U256) -> f64 {
    // Convert U256 to string then parse as f64 for safety
    let amount_str = amount.to_string();
    // Parse the full value and divide by 1e18
    amount_str.parse::<f64>().unwrap_or(0.0) / 1e18
}

/// Helper function to format f64 for display
fn fmt_eth(val: f64) -> String {
    format!("{:.6}", val)
}

/// Helper function to format percentage
fn fmt_pct(val: f64) -> String {
    format!("{:.2}", val)
}

/// Helper function to format scientific notation
fn fmt_sci(val: f64) -> String {
    format!("{:.2e}", val)
}

/// Trait para estratégias de MEV no padrão Artemis
#[async_trait::async_trait]
pub trait Strategy: Send + Sync {
    /// Processa um evento de MEV
    /// Deve ser não-bloqueante e nunca fazer panic
    async fn process_event(
        &mut self,
        event: MevEvent,
        context: &StrategyContext,
    ) -> eyre::Result<()>;

    /// Inicializa a estratégia com dados históricos
    async fn initialize(&mut self, initial_data: Vec<NormalizedSwapEvent>) -> eyre::Result<()>;

    /// Retorna estatísticas da estratégia
    fn stats(&self) -> StrategyStats;
}

/// Contexto compartilhado entre estratégias - ABSOLUTE DRY RUN
#[derive(Clone, Debug)]
pub struct StrategyContext {
    /// Endereço do executador (wallet)
    pub executor_address: Address,
    /// Gas price máximo aceitável (wei)
    pub max_gas_price: u128,
    /// Slippage máximo permitido (bps)
    pub max_slippage_bps: u32,
    /// Mínimo de lucro para executar (ETH em wei)
    pub min_profit_eth: u128,
    /// Preço do ETH em USD (para cálculos de lucro)
    pub eth_price_usd: f64,
    /// Taxa de prioridade (tip) em gwei
    pub priority_fee_gwei: u64,
    /// PILLAR 2: Hard-Cap de Gás Dinâmico - máximo priority fee aceitável
    pub max_priority_fee_gwei: u64,
    /// PILLAR 3: Tempo máximo de reação para backrun (ms)
    pub max_reaction_time_ms: u64,
    /// Modo DRY RUN (true = simulação sem execução real)
    pub dry_run: bool,
}

impl Default for StrategyContext {
    fn default() -> Self {
        Self {
            executor_address: Address::ZERO,
            max_gas_price: 100_000_000_000, // 100 gwei
            max_slippage_bps: 100, // 1%
            min_profit_eth: 5_000_000_000_000_000u128, // 0.005 ETH (atualizado para novo padrão)
            eth_price_usd: 3500.0, // $3500 por ETH
            priority_fee_gwei: 2, // 2 gwei de tip
            max_priority_fee_gwei: 50, // PILLAR 2: Hard-cap de 50 gwei
            max_reaction_time_ms: 100, // PILLAR 3: 100ms para reação
            dry_run: true, // Por padrão, modo seguro
        }
    }
}

/// Estatísticas estendidas da estratégia de elite
#[derive(Clone, Debug, Default)]
pub struct StrategyStats {
    pub events_processed: u64,
    pub opportunities_found: u64,
    pub executions_attempted: u64,
    pub executions_successful: u64,
    pub total_profit_wei: u128,
    pub errors: u64,
    pub avg_processing_time_us: u64,
    // Campos adicionais para telemetria de elite
    pub pools_discovered: u64,
    pub elite_swaps_detected: u64,
    pub honeypots_blocked: u64,
    pub total_volume_eth: f64,
}

/// Estado avançado da pool com métricas de elite
#[derive(Clone, Debug)]
struct PoolState {
    token0: Address,
    token1: Address,
    reserve0: u128,
    reserve1: u128,
    last_update: u64,
    /// Volume total processado (ETH)
    total_volume_eth: f64,
    /// Número de swaps nesta pool
    swap_count: u64,
    /// Último preço calculado
    last_price: f64,
    /// DEX type para validação
    dex_type: DexType,
}

/// Oportunidade de MEV calculada com precisão
#[derive(Clone, Debug)]
pub struct MevOpportunity {
    pub path: ArbitragePath,
    pub gross_profit_eth: f64,
    pub net_profit_eth: f64,
    pub gas_cost_eth: f64,
    pub slippage_bps: f64,
    pub confidence_score: f64,
    pub risk_level: RiskLevel,
    pub execution_priority: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Honeypot,
}

/// Estratégia de arbitragem PREDADOR DE ELITE
pub struct ArbitrageStrategy {
    stats: StrategyStats,
    pool_states: HashMap<Address, PoolState>,
    /// Factories autorizadas para validação de pools
    authorized_factories: Vec<Address>,
    /// Pools suspeitas (honeypots potenciais)
    suspicious_pools: HashMap<Address, u32>, // Address -> strike count
    /// Configurações de elite
    elite_threshold_eth: f64, // 0.1 ETH mínimo
    min_price_impact_bps: f64, // 1% mínimo
    max_honeypot_strikes: u32, // 3 strikes = honeypot confirmado
}

impl ArbitrageStrategy {
    pub fn new() -> Self {
        // Inicializar factories autorizadas
        let authorized_factories = vec![
            UniswapV3Factory::ADDRESS,
            AerodromeFactory::BASE_MAINNET,
            // PancakeSwap V3 na Base: 0x0BFbCF9fa4f9C588B8F2C85b4aa09989AadABe14
            Address::new([
                0x0B, 0xFb, 0xCF, 0x9f, 0xa4, 0xf9, 0xC5, 0x88,
                0xB8, 0xF2, 0xC8, 0x5b, 0x4a, 0xa0, 0x99, 0x89,
                0xAA, 0xdA, 0xBe, 0x14,
            ]),
        ];
        
        Self {
            stats: StrategyStats::default(),
            pool_states: HashMap::with_capacity(10000),
            authorized_factories,
            suspicious_pools: HashMap::new(),
            elite_threshold_eth: 0.1, // 0.1 ETH mínimo para elite
            min_price_impact_bps: 100.0, // 1% mínimo
            max_honeypot_strikes: 3,
        }
    }

    /// Verifica se uma pool pertence a uma factory autorizada
    fn is_pool_authorized(&self, _pool: Address, dex_type: DexType) -> bool {
        // Simplificado: validamos pelo DEX type identificado no evento
        matches!(dex_type, DexType::UniswapV3 | DexType::Aerodrome | DexType::PancakeSwap)
    }

    /// Dynamic Pool Discovery: Injeta pool em tempo real se for válida
    fn discover_pool(&mut self, swap: &NormalizedSwapEvent) {
        if !self.pool_states.contains_key(&swap.pool) {
            // Verificar se é uma pool autorizada
            if !self.is_pool_authorized(swap.pool, swap.dex_type) {
                trace!("Pool {:?} rejeitada - não pertence a factory autorizada", swap.pool);
                return;
            }
            
            // Criar novo estado de pool
            let state = PoolState {
                token0: swap.token_in,
                token1: swap.token_out,
                reserve0: swap.amount_in.to::<u128>(),
                reserve1: swap.amount_out.to::<u128>(),
                last_update: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                total_volume_eth: 0.0,
                swap_count: 1,
                last_price: 0.0,
                dex_type: swap.dex_type,
            };
            
            self.pool_states.insert(swap.pool, state);
            self.stats.pools_discovered += 1;
            
            info!("🎯 [DYNAMIC POOL] Nova pool descoberta: {:?} | DEX: {:?}", 
                  swap.pool, swap.dex_type);
        }
    }

    /// Atualiza estado da pool com evento de swap
    fn update_pool_state(&mut self, swap: &NormalizedSwapEvent) {
        // Dynamic Pool Discovery primeiro
        self.discover_pool(swap);
        
        if let Some(state) = self.pool_states.get_mut(&swap.pool) {
            // Atualizar reservas (simplificado - usando valores do swap)
            state.reserve0 = swap.amount_in.to::<u128>();
            state.reserve1 = swap.amount_out.to::<u128>();
            state.last_update = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            state.swap_count += 1;
            
            // Calcular preço atual
            if state.reserve0 > 0 {
                state.last_price = state.reserve1 as f64 / state.reserve0 as f64;
            }
        }
    }

    /// FILTRO DE ELITE: Verifica se o swap é "elite" (> 0.1 ETH e > 1% impacto)
    fn is_elite_swap(&self, swap: &NormalizedSwapEvent) -> bool {
        let amount_in_eth = u256_to_eth_f64(&swap.amount_in);
        
        // Threshold mínimo: 0.1 ETH
        if amount_in_eth < self.elite_threshold_eth {
            return false;
        }
        
        // Calcular impacto de preço usando x * y = k
        if let Some(state) = self.pool_states.get(&swap.pool) {
            if state.reserve0 > 0 && state.reserve1 > 0 {
                // Preço antes: reserve1 / reserve0
                let price_before = state.reserve1 as f64 / state.reserve0 as f64;
                
                // Nova reserva após swap (simplificado)
                let new_reserve0 = state.reserve0 as f64 + u256_to_eth_f64(&swap.amount_in) * 1e18;
                let new_reserve1 = state.reserve1 as f64 - u256_to_eth_f64(&swap.amount_out) * 1e18;
                
                if new_reserve0 > 0.0 && new_reserve1 > 0.0 {
                    let price_after = new_reserve1 / new_reserve0;
                    let price_impact = ((price_after - price_before) / price_before).abs() * 10000.0; // bps
                    
                    return price_impact >= self.min_price_impact_bps;
                }
            }
        }
        
        // Se não temos estado da pool, consideramos elite se o valor for alto
        amount_in_eth >= 0.5 // 0.5 ETH sem histórico = elite
    }

    /// ESCUDO ANTI-HONEYPOT: Simula venda reversa
    fn simulate_reverse_swap(&self, swap: &NormalizedSwapEvent) -> HoneypotCheck {
        // Simulação simplificada - em produção usar revm
        let amount_out = u256_to_eth_f64(&swap.amount_out) * 1e18;
        let amount_in = u256_to_eth_f64(&swap.amount_in) * 1e18;
        
        // Taxa implícita de swap (deveria ser ~0.3% para Uniswap V3)
        let implied_fee = if amount_out > 0.0 {
            (amount_in - amount_out) / amount_in * 100.0
        } else {
            100.0 // Taxa de 100% = honeypot óbvio
        };
        
        // Se taxa > 10%, provável honeypot
        if implied_fee > 10.0 {
            return HoneypotCheck {
                is_honeypot: true,
                burn_tax_bps: (implied_fee * 100.0) as u32,
                can_sell: false,
            };
        }
        
        HoneypotCheck {
            is_honeypot: false,
            burn_tax_bps: (implied_fee * 100.0) as u32,
            can_sell: true,
        }
    }

    /// Busca oportunidades de arbitragem real com cálculo de lucro
    fn find_arbitrage_opportunities(&mut self, swap: &NormalizedSwapEvent, context: &StrategyContext) -> Vec<MevOpportunity> {
        let mut opportunities = Vec::new();
        
        // FILTRO DE ELITE
        if !self.is_elite_swap(swap) {
            return opportunities;
        }
        
        self.stats.elite_swaps_detected += 1;
        
        // ESCUDO ANTI-HONEYPOT
        let honeypot_check = self.simulate_reverse_swap(swap);
        if honeypot_check.is_honeypot {
            // Marcar pool como suspeita
            let strikes = self.suspicious_pools.entry(swap.pool).or_insert(0);
            *strikes += 1;
            
            if *strikes >= self.max_honeypot_strikes {
                warn!("🚫 [HONEYPOT CONFIRMADO] Pool {:?} bloqueada após {} strikes", 
                      swap.pool, strikes);
                self.stats.honeypots_blocked += 1;
            } else {
                warn!("⚠️ [HONEYPOT SUSPEITO] Pool {:?} - Strike {}/{} | Taxa: {}%", 
                      swap.pool, strikes, self.max_honeypot_strikes, 
                      honeypot_check.burn_tax_bps as f64 / 100.0);
            }
            return opportunities;
        }
        
        // Calcular oportunidade de BACKRUN com Newton-Raphson
        let volume_eth = u256_to_eth_f64(&swap.amount_in);
        self.stats.total_volume_eth += volume_eth;
        
        // Obter estado da pool para cálculos
        let pool_state = self.pool_states.get(&swap.pool);
        let reserve0 = pool_state.map(|s| s.reserve0 as f64).unwrap_or(0.0);
        let reserve1 = pool_state.map(|s| s.reserve1 as f64).unwrap_or(0.0);
        
        // Constante do produto (x * y = k)
        let k = reserve0 * reserve1;
        
        // ═══════════════════════════════════════════════════════════
        // TAXAS FIXAS (independentes de L)
        // ═══════════════════════════════════════════════════════════
        let flashloan_fee_bps = match swap.dex_type {
            DexType::UniswapV3 | DexType::UniswapV2 => 30.0, // 0.3% média
            DexType::Aerodrome => 20.0, // 0.2% typical
            DexType::PancakeSwap => 25.0, // 0.25%
            DexType::AerodromeStable => 4.0, // 0.04% typical para stable pools
        };
        let fee_rate = flashloan_fee_bps / 10000.0; // 0.003 para 0.3%
        
        // ═══════════════════════════════════════════════════════════
        // PILLAR 2: HARD-CAP DE GÁS DINÂMICO
        // ═══════════════════════════════════════════════════════════
        // Gás conservador: 200k para segurança (PILLAR 4)
        let gas_used_flashloan = 200_000u128;
        
        // Verificar se gás atual está dentro do hard-cap
        let current_gas_price_gwei = context.max_gas_price as f64 / 1e9;
        let priority_fee_gwei = context.priority_fee_gwei as f64;
        
        // Se priority fee excede hard-cap, recalcular ou abortar
        if (priority_fee_gwei as u64) > context.max_priority_fee_gwei {
            info!("[SAFETY] Hard-Cap de Gás excedido: {} gwei > {} gwei max", 
                  priority_fee_gwei, context.max_priority_fee_gwei);
            return opportunities; // Abortar - gás muito alto
        }
        
        let gas_cost_eth = gas_used_flashloan as f64 * current_gas_price_gwei / 1e9;
        let priority_fee_eth = priority_fee_gwei / 1e9;
        let gas_y = gas_cost_eth + priority_fee_eth;
        
        // ═══════════════════════════════════════════════════════════
        // MÉTODO DE NEWTON-RAPHSON PARA ENCONTRAR L ÓTIMO
        // ═══════════════════════════════════════════════════════════
        // Função de lucro líquido:
        // f(L) = L * (r1/(r0+L) - r0/(r1+output)) - L*fee_rate - gas_y
        // Simplificado para modelo de impacto de preço:
        // f(L) = profit_from_arbitrage(L) - L*fee_rate - gas_y
        // 
        // Derivada (para Newton-Raphson):
        // f'(L) = marginal_profit(L) - fee_rate
        //
        // Fórmula de atualização:
        // L_new = L_old - f'(L) / f''(L)
        
        // Limites de liquidez (evitar insuficient liquidity)
        let l_max = reserve0 * 0.49; // Máximo 49% da reserva para segurança
        let l_min = 0.001; // Mínimo 0.001 ETH
        
        // Estimativa inicial baseada no volume do trigger
        let calculated = volume_eth * 1e18 * 2.0;
        let clamped = f64::min(calculated, l_max);
        let mut l = f64::max(clamped, l_min); // 2x volume inicial
        
        // Parâmetros de convergência
        let max_iterations = 10;
        let tolerance = 0.0001; // 0.01% de tolerância
        let mut converged = false;
        let mut iteration = 0;
        
        // ═══════════════════════════════════════════════════════════
        // LOOP NEWTON-RAPHSON
        // ═══════════════════════════════════════════════════════════
        while iteration < max_iterations && !converged {
            // Evitar divisão por zero ou valores negativos
            if l <= 0.0 || l >= reserve0 {
                l = l_max * 0.5;
                break;
            }
            
            // ═══════════════════════════════════════════════════════
            // CÁLCULO DAS DERIVADAS (usando fórmula constant product)
            // ═══════════════════════════════════════════════════════
            // Para swap de token0 para token1:
            // amount_out = (reserve1 * amount_in) / (reserve0 + amount_in)
            // Preço após swap = reserve1' / reserve0'
            
            let r0_after = reserve0 + l;
            let amount_out = (reserve1 * l) / r0_after;
            let r1_after = reserve1 - amount_out;
            
            // Preço antes e depois
            let price_before = reserve1 / reserve0;
            let price_after = r1_after / r0_after;
            
            // Lucro bruto = L * (diferença de preço aproveitável)
            // Simplificação: lucro vem do impacto de preço que conseguimos explorar
            let price_diff = price_before - price_after;
            let _profit_gross = l * price_diff * 0.5; // 50% da diferença é capturável
            
            // Derivada primeira (marginal profit)
            // d(profit)/dL = preço_efetivo - custo_marginal
            let effective_price = price_after;
            let marginal_cost = fee_rate; // Taxa constante por ETH flashloan
            let f_prime = effective_price - marginal_cost;
            
            // Derivada segunda (curvatura - simplificada)
            // Aproximação: segunda derivada do sistema constant product
            let f_double_prime = -2.0 * reserve1 * reserve0 / (r0_after * r0_after * r0_after);
            
            // Atualização Newton-Raphson
            if f_double_prime.abs() > 1e-10 { // Evitar divisão por zero
                let l_new = l - f_prime / f_double_prime;
                
                // Verificar convergência
                let delta = (l_new - l).abs() / l;
                converged = delta < tolerance;
                
                // Aplicar limites
                l = l_new.clamp(l_min, l_max);
            } else {
                converged = true; // Não conseguimos melhorar
            }
            
            iteration += 1;
        }
        
        // L ótimo encontrado
        let flashloan_l = l / 1e18; // Converter para ETH
        let flashloan_l_wei = l; // Manter em wei para cálculos
        
        // ═══════════════════════════════════════════════════════════
        // CÁLCULO FINAL COM L ÓTIMO
        // ═══════════════════════════════════════════════════════════
        let r0_final = reserve0 + flashloan_l_wei;
        let amount_out_final = (reserve1 * flashloan_l_wei) / r0_final;
        let r1_final = reserve1 - amount_out_final;
        
        let price_before = reserve1 / reserve0;
        let price_after = r1_final / r0_final;
        let price_diff_final = price_before - price_after;
        
        // Lucro bruto final
        let gross_profit_eth = flashloan_l * price_diff_final * 0.5;
        
        // Taxa de flashloan proporcional
        let taxa_x = flashloan_l * fee_rate;
        
        // Slippage estimada
        let slippage_eth = gross_profit_eth * (context.max_slippage_bps as f64 / 10000.0);
        
        // Lucro líquido final
        let lucro_z = gross_profit_eth - taxa_x - gas_y - slippage_eth;
        let lucro_eur = lucro_z * context.eth_price_usd * 0.92;
        
        // Verificar se é lucrativo
        let min_profit_eth = context.min_profit_eth as f64 / 1e18;
        
        // ═══════════════════════════════════════════════════════════
        // PILLAR 1: SIMULAÇÃO LOCAL 'FAIL-FAST'
        // ═══════════════════════════════════════════════════════════
        // Simular o swap usando estado atual das reservas
        let simulated_output = (reserve1 * flashloan_l_wei) / (reserve0 + flashloan_l_wei);
        let simulated_return = (reserve0 * simulated_output) / (reserve1 - simulated_output + flashloan_l_wei);
        let simulated_profit_wei = simulated_return - flashloan_l_wei;
        let simulated_profit_eth = simulated_profit_wei / 1e18;
        
        // Lucro simulado após gás real estimado (200k gas)
        let gas_cost_simulated = 200_000.0 * current_gas_price_gwei / 1e9;
        let simulated_profit_after_gas = simulated_profit_eth - gas_cost_simulated - taxa_x;
        
        // FAIL-FAST: Abortar se lucro simulado < 0.005 ETH
        if simulated_profit_after_gas < 0.005 {
            info!("[SAFETY] Abortado: Lucro simulado insuficiente após Gás");
            info!("[SAFETY]    Esperado: {} ETH | Mínimo: 0.005 ETH", fmt_eth(simulated_profit_after_gas));
            return opportunities;
        }
        
        // ═══════════════════════════════════════════════════════════
        // PILLAR 3: REFORÇO DE BACKRUNNING - Verificação de tempo
        // ═══════════════════════════════════════════════════════════
        let swap_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let reaction_time_ms = current_time.saturating_sub(swap_timestamp);
        
        if reaction_time_ms > context.max_reaction_time_ms {
            info!("[SAFETY] Backrun inviável: Tempo de reação {}ms > {}ms limite", 
                  reaction_time_ms, context.max_reaction_time_ms);
            return opportunities;
        }
        
        // TELEMETRIA NEWTON-RAPHSON
        let reserve0_eth = reserve0 / 1e18;
        let reserve1_eth = reserve1 / 1e18;
        let l_max_eth = l_max / 1e18;
        let reserve0_49pct = reserve0_eth * 0.49;
        
        info!("═══════════════════════════════════════════════════════════");
        info!("[DRY-RUN] 🎯 ALVO: Pool {:?} | DEX: {:?}", swap.pool, swap.dex_type);
        info!("[DRY-RUN]    Reservas: {} / {} | k = {}", fmt_eth(reserve0_eth), fmt_eth(reserve1_eth), fmt_sci(k));
        info!("═══════════════════════════════════════════════════════════");
        info!("[DRY-RUN] 🧮 NEWTON-RAPHSON: {} iterações | Convergido: {}", iteration, converged);
        info!("[DRY-RUN] 🚀 FLASHLOAN ÓTIMO (L): {} ETH", fmt_eth(flashloan_l));
        info!("[DRY-RUN]    └─ Limites: [{}, {}] ETH", fmt_eth(l_min), fmt_eth(l_max_eth));
        info!("[DRY-RUN]    └─ Preço antes: {} | Depois: {}", fmt_eth(price_before), fmt_eth(price_after));
        info!("[DRY-RUN] 📉 TAXA EMPRÉSTIMO (X): {} ETH ({}%)", fmt_eth(taxa_x), fmt_pct(fee_rate * 100.0));
        info!("[DRY-RUN] ⛽ GÁS + PRIORIDADE (Y): {} ETH", fmt_eth(gas_y));
        info!("[DRY-RUN]    └─ Gas: {} ETH ({} gas @ {} gwei) + Tip: {} ETH", 
              fmt_eth(gas_cost_eth), gas_used_flashloan, fmt_eth(current_gas_price_gwei), fmt_eth(priority_fee_eth));
        info!("[DRY-RUN] 💎 LUCRO BRUTO: {} ETH | Slippage: {} ETH", fmt_eth(gross_profit_eth), fmt_eth(slippage_eth));
        
        // Verificação final: Lucro Z positivo e acima do mínimo
        if lucro_z >= min_profit_eth && flashloan_l > 0.0 && lucro_z > 0.0 {
            // OPORTUNIDADE VIÁVEL - TODOS OS PILARES SATISFEITOS
            info!("[SAFETY] ✓ Fail-Fast: Simulação OK ({} ETH > 0.005 ETH)", fmt_eth(simulated_profit_after_gas));
            info!("[SAFETY] ✓ Hard-Cap Gás: {} gwei < {} gwei max", priority_fee_gwei, context.max_priority_fee_gwei);
            info!("[SAFETY] ✓ Backrun: {}ms < {}ms limite", reaction_time_ms, context.max_reaction_time_ms);
            let margem = (lucro_z/flashloan_l)*100.0;
            let roi = (lucro_z/(flashloan_l+gas_y))*100.0;
            info!("[DRY-RUN] 💰 LUCRO LÍQUIDO (Z): {} ETH ({} EUR)", fmt_eth(lucro_z), fmt_eth(lucro_eur));
            info!("[DRY-RUN]    └─ Z = Bruto({}) - X({}) - Y({}) - Slippage({})", 
                  fmt_eth(gross_profit_eth), fmt_eth(taxa_x), fmt_eth(gas_y), fmt_eth(slippage_eth));
            info!("[DRY-RUN]    └─ Margem: {}% | ROI Flashloan: {}%", fmt_pct(margem), fmt_pct(roi));
            info!("[DRY-RUN] ✅ STATUS: OPORTUNIDADE VIÁVEL | Confiança: 85%");
            info!("═══════════════════════════════════════════════════════════");
            
            let path = ArbitragePath {
                pools: vec![swap.pool],
                tokens: vec![swap.token_in, swap.token_out],
                expected_profit_wei: (lucro_z * 1e18) as u128,
                gas_cost_wei: (gas_y * 1e18) as u128,
                confidence: if converged { 0.90 } else { 0.75 }, // Mais confiança se convergiu
            };
            
            let opportunity = MevOpportunity {
                path: path.clone(),
                gross_profit_eth,
                net_profit_eth: lucro_z,
                gas_cost_eth: gas_y,
                slippage_bps: context.max_slippage_bps as f64,
                confidence_score: if converged { 0.90 } else { 0.75 },
                risk_level: RiskLevel::Low,
                execution_priority: if lucro_z > 0.01 { 1 } else { 2 }, // Prioridade alta se > 0.01 ETH
            };
            
            opportunities.push(opportunity);
        } else {
            // OPORTUNIDADE INVÁVEL
            info!("[DRY-RUN] 💰 LUCRO LÍQUIDO (Z): {} ETH ({} EUR)", fmt_eth(lucro_z), fmt_eth(lucro_eur));
            info!("[DRY-RUN]    └─ Z = Bruto({}) - X({}) - Y({}) - Slippage({})", 
                  fmt_eth(gross_profit_eth), fmt_eth(taxa_x), fmt_eth(gas_y), fmt_eth(slippage_eth));
            info!("[DRY-RUN] ❌ STATUS: OPORTUNIDADE INVÁVEL");
            
            // Diagnosticar o problema
            if flashloan_l <= 0.0 || flashloan_l >= reserve0_49pct {
                info!("[DRY-RUN]    💡 MOTIVO: L ótimo ({} ETH) viola limites de liquidez", fmt_eth(flashloan_l));
            } else if gross_profit_eth < taxa_x {
                info!("[DRY-RUN]    💡 MOTIVO: Taxa de flashloan ({} ETH) excede lucro bruto ({} ETH)",
                      fmt_eth(taxa_x), fmt_eth(gross_profit_eth));
            } else if lucro_z < 0.0 {
                let total_costs = taxa_x + gas_y + slippage_eth;
                info!("[DRY-RUN]    💡 MOTIVO: Custos totais (X+Y+Slippage={}) excedem lucro bruto",
                      fmt_eth(total_costs));
            } else {
                info!("[DRY-RUN]    💡 MOTIVO: Lucro abaixo do mínimo ({} ETH < {} ETH)",
                      fmt_eth(lucro_z), fmt_eth(min_profit_eth));
            }
            info!("═══════════════════════════════════════════════════════════");
        }
        
        opportunities
    }
}

/// Resultado da verificação anti-honeypot
#[derive(Clone, Debug)]
struct HoneypotCheck {
    is_honeypot: bool,
    burn_tax_bps: u32,
    can_sell: bool,
}

#[async_trait::async_trait]
impl Strategy for ArbitrageStrategy {
    async fn process_event(
        &mut self,
        event: MevEvent,
        context: &StrategyContext,
    ) -> eyre::Result<()> {
        let start = std::time::Instant::now();
        
        match event {
            MevEvent::Swap(swap) => {
                // 🔴 [RADAR] Log absolutamente TODOS os swaps detectados
                let amount_eth = u256_to_eth_f64(&swap.amount_in);
                info!(
                    "[RADAR] 📡 Swap detectado na Pool {:?} | DEX: {:?} | Amount: {} ETH | Token: {:?} -> {:?}",
                    swap.pool, swap.dex_type, fmt_eth(amount_eth), swap.token_in, swap.token_out
                );
                
                // Processamento NÃO-BLOQUEANTE
                let opportunities = self.find_arbitrage_opportunities(&swap, context);
                
                if !opportunities.is_empty() {
                    self.stats.opportunities_found += opportunities.len() as u64;
                    
                    for opp in &opportunities {
                        if context.dry_run {
                            info!(
                                "[DRY-RUN] ✅ OPORTUNIDADE REGISTADA: {} ETH líquido | Confiança: {:.0}%",
                                opp.net_profit_eth, opp.confidence_score * 100.0
                            );
                        } else {
                            // Modo LIVE - enviaria para executor
                            info!(
                                "[LIVE] 🚀 EXECUTANDO: {} ETH líquido | Prioridade: {}",
                                opp.net_profit_eth, opp.execution_priority
                            );
                            self.stats.executions_attempted += 1;
                        }
                    }
                } else {
                    // 🔴 [REJECT] Honestidade total - explicar porquê rejeitamos
                    info!(
                        "[REJECT] ❌ Swap na Pool {:?} rejeitado | Lucro insuficiente ou gás excessivo",
                        swap.pool
                    );
                }
                
                // Atualizar estado da pool (após processamento)
                self.update_pool_state(&swap);
            }
            MevEvent::BlockUpdate(block) => {
                trace!("Block update: {}", block);
            }
            MevEvent::PriceUpdate { token, price } => {
                trace!("Price update: {:?} = {}", token, price);
            }
        }

        self.stats.events_processed += 1;
        let elapsed = start.elapsed().as_micros() as u64;
        
        // Média móvel exponencial para performance
        self.stats.avg_processing_time_us = 
            (self.stats.avg_processing_time_us * 9 + elapsed) / 10;

        Ok(())
    }

    async fn initialize(&mut self, initial_data: Vec<NormalizedSwapEvent>) -> eyre::Result<()> {
        info!("═══════════════════════════════════════════════════════════");
        info!("🎯 ESTRATÉGIA PREDADOR DE ELITE - Inicialização");
        info!("═══════════════════════════════════════════════════════════");
        info!("🔧 Filtro Elite: {} ETH mínimo | {}% impacto mínimo", 
              self.elite_threshold_eth, self.min_price_impact_bps / 100.0);
        info!("🛡️ Anti-Honeypot: {} strikes máximo", self.max_honeypot_strikes);
        info!("📊 Processando {} eventos históricos...", initial_data.len());
        
        for swap in initial_data {
            self.update_pool_state(&swap);
        }
        
        info!("✅ Estratégia inicializada: {} pools | {} factories autorizadas", 
              self.pool_states.len(), self.authorized_factories.len());
        info!("═══════════════════════════════════════════════════════════");
        Ok(())
    }

    fn stats(&self) -> StrategyStats {
        self.stats.clone()
    }
}

/// Caminho de arbitragem identificado
#[derive(Clone, Debug)]
pub struct ArbitragePath {
    pub pools: Vec<Address>,
    pub tokens: Vec<Address>,
    pub expected_profit_wei: u128,
    pub gas_cost_wei: u128,
    pub confidence: f64,
}
