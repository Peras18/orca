//! 💰 BankrollManager — Gestão adaptativa da banca para MEV
//!
//! Três camadas de proteção:
//! 1. Gas Budget dinâmico (máximo 3% da banca por dia)
//! 2. Flash Loan sizing proporcional à banca
//! 3. Profit threshold adaptativo

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Gere todos os parâmetros financeiros do bot de forma adaptativa.
/// Todos os valores escalam automaticamente com a banca actual.
#[derive(Debug)]
pub struct BankrollManager {
    /// Banca actual em wei (actualizada a cada bloco)
    pub current_balance: u128,
    /// Histórico de lucro/perda das últimas 100 txns
    win_history: VecDeque<i128>, // positivo = lucro, negativo = perda
    /// Número de txns falhadas consecutivas (reset-se em cada sucesso)
    /// 🚨 Usa AtomicU32 para thread-safety
    pub consecutive_failures: Arc<AtomicU32>,
}

impl BankrollManager {
    pub fn new(initial_balance_wei: u128) -> Self {
        Self {
            current_balance: initial_balance_wei,
            win_history: VecDeque::with_capacity(100),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
        }
    }

    /// CAMADA 1 — Gas budget máximo por sessão de 24h.
    /// Nunca gastas mais que 3% da banca em gas num dia.
    /// Com 80€ (~0.05 ETH): máximo 0.0015 ETH em gas/dia.
    /// Com 1 ETH: máximo 0.03 ETH em gas/dia.
    pub fn max_daily_gas_budget(&self) -> u128 {
        self.current_balance * 3 / 100
    }

    /// CAMADA 2 — Flash loan amount óptimo para esta banca.
    /// A lógica: quanto maior a banca, mais podes pedir
    /// porque tens mais buffer para absorver falhas.
    ///
    /// Escala logarítmica — não linear — para evitar
    /// crescimento demasiado agressivo nas fases iniciais.
    ///
    /// `pool_reserve_in` DEVE ser a reserve real do pool alvo (18-dec), não
    /// um valor sintético derivado de `swap.amount_in`.  Se passar um valor
    /// sintético pequeno (ex. amount_in × 20) o cap de 15% pode produzir
    /// flash amounts na ordem de milhões de wei — demasiado pequenos para
    /// a divisão inteira na fórmula AMM retornar resultado não-zero.
    pub fn optimal_flash_amount(&self, pool_reserve_in: u128) -> u128 {
        // Mínimo absoluto: 0.01 ETH.  Abaixo disto, a fórmula AMM arredonda
        // para zero em pools de alta liquidez (reserve ~350 ETH) porque
        // numerator < denominator na divisão inteira.
        // Derivado de:  min_in > (reserve_in × f_den) / (f_num × reserve_out)
        // Para WETH/USDC Aerodrome: ~4.3×10¹¹ wei; 0.01 ETH = 10¹⁶ >> este valor.
        const MIN_FLASH_WEI: u128 = 10_000_000_000_000_000; // 0.01 ETH

        // Banca em ETH (18 decimais)
        let balance_eth = self.current_balance / 1_000_000_000_000_000; // em finney

        // CORREÇÃO: o tamanho do flash loan NÃO precisa de escalar com a banca --
        // o risco de um flash loan é só o gás da tentativa (atómico: ou paga-se
        // de volta com lucro, ou a tx inteira reverte, nunca se "perde" o
        // principal emprestado). O travão real de segurança é o cap de 15% da
        // reserve da pool, calculado a seguir -- esse sim protege contra
        // destruir o lucro com slippage. Pedir sempre uma base alta deixa esse
        // cap fazer o trabalho de limitar, em vez de a banca pequena limitar
        // artificialmente oportunidades grandes que a pool aguentaria.
        let _ = balance_eth; // mantido para logging/diagnóstico, já não decide o tamanho
        let base_flash_eth: u128 = 10_000; // sempre tenta 10.0 ETH como base de partida

        let flash_wei = base_flash_eth * 1_000_000_000_000_000;

        // Nunca ultrapassar 15% da reserve real do pool
        // (impacto de preço destrói o lucro acima disto)
        let max_from_reserve = pool_reserve_in.saturating_mul(15) / 100;

        // Aplicar cap de reserve E garantir mínimo operacional.
        // `max(MIN_FLASH_WEI)` é aplicado por último para que mesmo pools
        // com reserve muito baixa (< 0.067 ETH) ainda tentem 0.01 ETH.
        flash_wei.min(max_from_reserve).max(MIN_FLASH_WEI)
    }

    /// CAMADA 3 — Lucro mínimo aceitável (em wei).
    /// Sobe com a banca para evitar executar oportunidades
    /// marginais quando tens mais a perder em falhas.
    ///
    /// Fórmula base: max(gas_cost * ratio_minimo, threshold_absoluto)
    /// Onde threshold_absoluto cresce com a banca.
    pub fn min_acceptable_profit(&self, estimated_gas_cost: u128, gas_price: u128) -> u128 {
        let gas_cost_wei = estimated_gas_cost * gas_price;

        // Ratio mínimo aumenta com a banca:
        // Banca pequena: aceitar ratio 2:1 (mais oportunidades, mais aprendizagem)
        // Banca média:   aceitar ratio 3:1 (equilibrado)
        // Banca grande:  aceitar ratio 5:1 (só as melhores)
        let min_ratio = self.min_profit_ratio();

        // Threshold absoluto: nunca menos que 0.01% da banca por txn
        // Garante que cada execução é material face ao capital em risco
        let absolute_threshold = self.current_balance / 10_000;

        let from_ratio = gas_cost_wei * min_ratio;

        from_ratio.max(absolute_threshold)
    }

    /// CAMADA 4 — Circuit breaker inteligente.
    /// Reduz agressividade automaticamente quando está a perder.
    /// Não precisa de intervenção manual.
    pub fn risk_multiplier(&self) -> f32 {
        let failures = self.consecutive_failures.load(Ordering::Relaxed);

        // Se tiver > 3 falhas consecutivas: reduzir para 50%
        // Se tiver > 5 falhas consecutivas: pausar (0%)
        // Se estiver a ganhar: 100% normal
        match failures {
            0..=2 => 1.0,
            3..=4 => 0.5,  // metade do flash loan, threshold duplo
            5..=6 => 0.25, // muito conservador
            _ => 0.0,      // parar — algo está fundamentalmente errado
        }
    }

    /// Ratio mínimo profit:gas baseado na banca actual.
    pub fn min_profit_ratio(&self) -> u128 {
        let balance_eth = self.current_balance / 1_000_000_000_000_000_000;
        match balance_eth {
            0 => 2,      // banca muito pequena: aceitar 2:1
            1..=4 => 3,  // banca média: 3:1
            5..=19 => 4, // banca boa: 4:1
            _ => 5,      // banca grande: só as melhores
        }
    }

    /// Registar resultado de uma txn (chamar após cada execução).
    pub fn record_result(&mut self, profit_or_loss: i128) {
        if self.win_history.len() >= 100 {
            self.win_history.pop_front();
        }
        self.win_history.push_back(profit_or_loss);

        if profit_or_loss > 0 {
            self.consecutive_failures.store(0, Ordering::Relaxed);
            self.current_balance = self.current_balance.saturating_add(profit_or_loss as u128);
        } else {
            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
            self.current_balance = self
                .current_balance
                .saturating_sub(profit_or_loss.unsigned_abs());
        }
    }

    /// Win rate das últimas N txns (entre 0.0 e 1.0).
    pub fn recent_win_rate(&self) -> f32 {
        if self.win_history.is_empty() {
            return 0.5;
        } // assumir 50% sem dados
        let wins = self.win_history.iter().filter(|&&p| p > 0).count();
        wins as f32 / self.win_history.len() as f32
    }

    /// Relatório de estado — chamar a cada 1000 blocos para log.
    pub fn status_report(&self) -> String {
        format!(
            "[BANKROLL] Banca: {} wei | Win rate: {:.1}% | Failures: {} | \
             Flash max: {} wei | Min profit: calculado por oportunidade",
            self.current_balance,
            self.recent_win_rate() * 100.0,
            self.consecutive_failures.load(Ordering::Relaxed),
            self.optimal_flash_amount(u128::MAX),
        )
    }

    /// 🚨 CORREÇÃO: Actualizar banca a cada N blocos via provider
    pub fn update_balance(&mut self, new_balance: u128) {
        self.current_balance = new_balance;
    }
}
