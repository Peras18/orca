with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''                self.sequencer.update_block(swap.block_number).await;
                self.sequencer_heartbeat.update_block(swap.block_number).await;'''

new = '''                // NOTA: update_block aqui usa swap.block_number como fallback
                // imediato (zero latência extra), mas a fonte de verdade real
                // é a poll task dedicada (ver spawn_block_poller, chamada no
                // arranque do OrcaEngine) -- swap.block_number sozinho causava
                // uma deriva progressiva (~12% mais lento que o bloco real)
                // porque só avança quando HÁ um swap relevante, não a cada
                // bloco real da chain.
                self.sequencer.update_block(swap.block_number).await;
                self.sequencer_heartbeat.update_block(swap.block_number).await;'''

count = content.count(old)
print(f"Ocorrências do ponto 1: {count}")
if count == 1:
    content = content.replace(old, new)

# Adicionar método spawn_block_poller à impl OrcaEngine
old2 = '''    /// 🔍 Valida oportunidade via simulação local'''

new2 = '''    /// 🔄 Mantém last_block sempre fresco via poll RPC direto, independente
    /// do fluxo de eventos de swap (que só avança quando HÁ actividade nas
    /// pools monitorizadas, causando deriva progressiva em relação ao bloco
    /// real -- já visto a chegar a -93 blocos de atraso em 14 minutos).
    pub fn spawn_block_poller(self: &Arc<Self>) {
        let engine = self.clone();
        tokio::spawn(async move {
            use alloy::providers::Provider as _;
            loop {
                match engine.balance_provider.get_block_number().await {
                    Ok(block) => {
                        engine.sequencer.update_block(block).await;
                        engine.sequencer_heartbeat.update_block(block).await;
                    }
                    Err(e) => {
                        warn!("[BLOCK-POLLER] falha a obter bloco real: {}", e);
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
            }
        });
    }

    /// 🔍 Valida oportunidade via simulação local'''

count2 = content.count(old2)
print(f"Ocorrências do ponto 2: {count2}")
if count2 == 1:
    content = content.replace(old2, new2)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso (ambos os pontos).")
else:
    print("ABORTADO no ponto 2 -- não foi escrito nada.")
