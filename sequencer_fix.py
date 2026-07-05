import sys

path = sys.argv[1]
with open(path, 'r') as f:
    content = f.read()

old_calc = '''    pub async fn calculate_optimal_timing(&self) -> BlockTiming {
        let rtt = *self.current_rtt_us.read().await;
        let stddev = *self.rtt_stddev.read().await;
        let last_block = *self.last_block.read().await;
        // Estimar próximo bloco
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let next_block = last_block + 1;
        let time_to_next = self.block_time_ms as i64 - (now as i64 % self.block_time_ms as i64);
        // Calcular janela de envio ótima
        // Enviar (RTT + margem) antes do bloco abrir
        let margin_us = 50_000u64; // 50ms margem de segurança
        let send_before_us = rtt + stddev + margin_us;
        // Verificar se ainda estamos na janela
        let can_be_top = time_to_next * 1000 > send_before_us as i64;
        // Ajustar priority fee baseado em competition
        let competition = self.estimate_competition().await;
        let base_fee = 0.1; // 0.1 gwei Base
        let priority_fee = base_fee * (1.0 + competition * 5.0); // 1x a 6x
        BlockTiming {
            target_block: next_block as u64,
            block_slot: if can_be_top { 0 } else { 150 },
            deadline: (now as i64 + time_to_next) as u64,
            will_be_top_of_block: can_be_top,
            priority_fee_gwei: priority_fee,
        }
    }'''

new_calc = '''    pub async fn calculate_optimal_timing(&self) -> BlockTiming {
        let rtt = *self.current_rtt_us.read().await;
        let stddev = *self.rtt_stddev.read().await;
        let last_block = *self.last_block.read().await;
        // Estimar próximo bloco
        // CORREÇÃO: 'deadline' é agora SEMPRE em milissegundos desde epoch
        // (era em segundos, mas multiplicado por 1000 em await_optimal_window
        // como se já estivesse em ms -- isso inflacionava o deadline real em
        // ~1000x, fazendo o bot "esperar" ~20min por bloco antes de cada envio,
        // perdendo sempre a janela e nunca executando nada).
        // 'now_ms % block_time_ms' agora usa as mesmas unidades (ms) dos
        // dois lados, dando o resto correto até ao próximo bloco.
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let next_block = last_block + 1;
        let time_to_next_ms = self.block_time_ms as i64 - (now_ms as i64 % self.block_time_ms as i64);
        // Calcular janela de envio ótima
        // Enviar (RTT + margem) antes do bloco abrir
        let margin_us = 50_000u64; // 50ms margem de segurança
        let send_before_us = rtt + stddev + margin_us;
        // Verificar se ainda estamos na janela
        let can_be_top = time_to_next_ms * 1000 > send_before_us as i64;
        // Ajustar priority fee baseado em competition
        let competition = self.estimate_competition().await;
        let base_fee = 0.1; // 0.1 gwei Base
        let priority_fee = base_fee * (1.0 + competition * 5.0); // 1x a 6x
        BlockTiming {
            target_block: next_block as u64,
            block_slot: if can_be_top { 0 } else { 150 },
            deadline: (now_ms as i64 + time_to_next_ms) as u64, // ms desde epoch
            will_be_top_of_block: can_be_top,
            priority_fee_gwei: priority_fee,
        }
    }'''

old_await = '''    pub async fn await_optimal_window(&self) -> BlockTiming {
        let timing = self.calculate_optimal_timing().await;
        let rtt_ms = (*self.current_rtt_us.read().await) / 1000;
        // Calcular quando enviar
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let send_at = timing.deadline * 1000 - rtt_ms - 100; // 100ms margem
        info!("[DIAG-SEQ] now={} deadline={} rtt_ms={} send_at={}", now, timing.deadline, rtt_ms, send_at);
        if send_at > now {
            let wait_ms = send_at - now;
            info!("[DIAG-SEQ] vai dormir {}ms ({:.2}h)", wait_ms, wait_ms as f64 / 3_600_000.0);
            trace!("[SEQUENCER] ⏳ Aguardando {}ms para envio", wait_ms);
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        timing
    }'''

new_await = '''    pub async fn await_optimal_window(&self) -> BlockTiming {
        let timing = self.calculate_optimal_timing().await;
        let rtt_ms = (*self.current_rtt_us.read().await) / 1000;
        // Calcular quando enviar
        // CORREÇÃO: 'timing.deadline' já vem em ms desde epoch (ver
        // calculate_optimal_timing) -- não multiplicar por 1000 outra vez.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let send_at = timing.deadline.saturating_sub(rtt_ms).saturating_sub(100); // 100ms margem
        if send_at > now {
            let wait_ms = (send_at - now).min(self.block_time_ms * 2); // nunca esperar mais que 2 blocos -- rede de segurança contra deadline mal calculado
            trace!("[SEQUENCER] ⏳ Aguardando {}ms para envio", wait_ms);
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        timing
    }'''

replacements = [(old_calc, new_calc, "calculate_optimal_timing"), (old_await, new_await, "await_optimal_window")]
all_ok = True
for old, new, name in replacements:
    count = content.count(old)
    print(f"{name}: {count} ocorrências")
    if count != 1:
        all_ok = False

if all_ok:
    for old, new, name in replacements:
        content = content.replace(old, new)
    with open(path, 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO -- alguma contagem != 1.")
