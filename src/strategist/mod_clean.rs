// Processamento de eventos - versao limpa
async fn process_event_clean(
    &mut self,
    event: MevEvent,
    _context: &StrategyContext,
) -> eyre::Result<()> {
    match event {
        MevEvent::Swap(swap) => {
            let start = std::time::Instant::now();
            
            // LOG: Confirmar recebimento do swap
            info!("✅ [SWAP RECEIVED] Pool: {:?} | TokenIn: {:?} | TokenOut: {:?} | Amount: {:?}",
                swap.pool, swap.token_in, swap.token_out, swap.amount_in);
            
            // Filtro de relevancia: ignorar swaps < 0.1 ETH
            let amount_eth = swap.amount_in.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
            if amount_eth < 0.1 {
                trace!("[SWAP FILTERED] Amount {:.6} ETH < 0.1 ETH threshold", amount_eth);
                return Ok(());
            }
            
            // Atualizar estado
            self.update_pool_from_swap(&swap).await;
            
            // Buscar oportunidades - prioriza 2-hop
            let paths = self.find_arbitrage_cycles(swap.token_in, 2).await;
            
            if paths.is_empty() {
                let elapsed_us = start.elapsed().as_micros() as u64;
                info!("[DNA-SCAN] Token: {:?} | Time: {}us | Status: No paths", 
                    swap.token_in, elapsed_us);
                return Ok(());
            }
            
            // Lock para operacoes
            let pools = self.pools.read().await;
            let mut opportunity_found = false;
            
            // Processar top 5 oportunidades
            for path in paths.iter().take(5) {
                match self.optimize_amount_newton_raphson(path, &pools) {
                    Some(newton_result) => {
                        // Simulacao atomica
                        let executor_addr = alloy::primitives::address!("0x1111111111111111111111111111111111111111");
                        let elite_result = self.elite_hunter.read().await.simulate_atomic_arbitrage(
                            path,
                            executor_addr,
                            50_000_000_000u128,
                        ).await;
                        
                        if !elite_result.success {
                            continue;
                        }
                        
                        // Validacao flashloan
                        let liquidity = U256::from(1_000_000_000_000_000_000u64);
                        let flash_calc = self.flash_strategy.read().await.calculate_optimal_loan(path, liquidity);
                        
                        let total_cost = flash_calc.flash_loan_fee_wei + flash_calc.total_gas_cost_wei;
                        let min_profit = total_cost + U256::from(5_000_000_000_000_000u128);
                        
                        let profit_eth = flash_calc.net_profit_wei.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                        let cost_eth = total_cost.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                        let is_profitable = flash_calc.net_profit_wei >= min_profit;
                        
                        if is_profitable {
                            info!("[OPPORTUNITY] Profit: {:.6} ETH | Cost: {:.6} ETH | Hops: {}",
                                profit_eth, cost_eth, path.hops.len());
                            opportunity_found = true;
                        }
                    }
                    None => {
                        trace!("Newton-Raphson failed for path with {} hops", path.hops.len());
                    }
                }
            }
            
            // Log tempo de processamento real
            let elapsed_us = start.elapsed().as_micros() as u64;
            info!("[DNA-SCAN] Token: {:?} | Paths: {} | Time: {}us | Found: {}",
                swap.token_in, paths.len(), elapsed_us, opportunity_found);
            
            // Atualizar estatisticas
            let mut stats = self.stats.write().await;
            stats.events_processed += 1;
            stats.pools_tracked = self.pools.read().await.len();
            if opportunity_found {
                stats.opportunities_found += 1;
            }
            stats.paths_evaluated += 1;
            stats.avg_processing_time_us = (stats.avg_processing_time_us + elapsed_us) / 2;
        }
        MevEvent::BlockUpdate(block) => {
            trace!("BlockUpdate: {} ({:?})", block.number, block.hash);
        }
        MevEvent::PendingTransaction(tx) => {
            trace!("PendingTransaction: {:?}", tx.hash);
        }
    }
    
    Ok(())
}
