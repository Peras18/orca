use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, info, trace};

use crate::types::{MempoolTx, Opportunity, OpportunityKind, Pool};
use crate::{EngineConfig, Pathfinder, Provider, Simulator};

const POLL_INTERVAL_MS: u64 = 50;
const OPPORTUNITY_BATCH_SIZE: usize = 64;

pub struct Engine {
    provider: Arc<Provider>,
    pathfinder: Arc<Pathfinder>,
    simulator: Arc<Simulator>,
    config: EngineConfig,
    pending_opportunities: Arc<RwLock<Vec<Opportunity>>>,
}

impl Engine {
    pub fn new(
        provider: Arc<Provider>,
        pathfinder: Arc<Pathfinder>,
        simulator: Arc<Simulator>,
        config: EngineConfig,
    ) -> Self {
        Self {
            provider,
            pathfinder,
            simulator,
            config,
            pending_opportunities: Arc::new(RwLock::new(Vec::with_capacity(OPPORTUNITY_BATCH_SIZE))),
        }
    }

    pub async fn run(&self) -> eyre::Result<()> {
        info!("ApexBaseMEV Engine initialized");
        info!("Region: {}", self.config.region);
        info!("Max path length: {}", self.config.max_path_length);
        info!("Min profit basis points: {}", self.config.min_profit_basis_points);

        self.initialize_pools().await?;

        let mut ticker = interval(Duration::from_millis(POLL_INTERVAL_MS));

        loop {
            tokio::select! {
                biased;
                
                _ = ticker.tick() => {
                    self.process_cycle().await?;
                }
            }
        }
    }

    async fn initialize_pools(&self) -> eyre::Result<()> {
        info!("Initializing liquidity pools...");
        
        let weth = alloy::primitives::address!("0x4200000000000000000000000000000000000006");
        let usdc = alloy::primitives::address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
        
        use crate::contracts::DexType;
        
        let pools = vec![
            Pool {
                address: alloy::primitives::address!("0x0000000000000000000000000000000000000000"),
                token_a: weth,
                token_b: usdc,
                fee: 500,
                reserve_a: alloy::primitives::U256::from(1000000000000000000000u128),
                reserve_b: alloy::primitives::U256::from(2000000000000u128),
                dex_type: DexType::UniswapV3,
            },
        ];

        for pool in pools {
            self.pathfinder.add_pool(pool);
        }

        info!("Pool initialization complete");
        Ok(())
    }

    async fn process_cycle(&self) -> eyre::Result<()> {
        let pending_txs = self.provider.get_pending_txs();
        
        if !pending_txs.is_empty() {
            trace!("Processing {} pending transactions", pending_txs.len());
            
            for tx in pending_txs {
                if let Err(e) = self.process_transaction(&tx).await {
                    trace!("Transaction processing error: {}", e);
                }
                
                self.scan_for_arbitrage(&tx).await;
            }
        }

        self.process_opportunities().await?;
        
        Ok(())
    }

    async fn process_transaction(&self, tx: &MempoolTx) -> eyre::Result<()> {
        let sim_result = self.simulator.simulate_pending_tx(tx).await?;
        
        if sim_result.is_honeypot {
            trace!("Transaction {:?} is honeypot - skipped", tx.hash);
            return Ok(());
        }

        if sim_result.success {
            debug!(
                "Transaction {:?} succeeded: gas_used={}, output={}",
                tx.hash, sim_result.gas_used, sim_result.output_amount
            );
            
            if let Some(opportunity) = self.detect_backrun_opportunity(tx, &sim_result).await? {
                self.enqueue_opportunity(opportunity).await;
            }
        }

        Ok(())
    }

    async fn detect_backrun_opportunity(
        &self,
        tx: &MempoolTx,
        _sim_result: &crate::types::SimulationResult,
    ) -> eyre::Result<Option<Opportunity>> {
        let trigger_state = self.simulator.clone_state().await;
        
        let weth = alloy::primitives::address!("0x4200000000000000000000000000000000000006");
        
        if let Some(path) = self.pathfinder.find_arbitrage(weth) {
            let backrun_result = self.simulator.execute_backrun(&path, &trigger_state).await?;
            
            if backrun_result.success && !backrun_result.is_honeypot {
                let expected_profit_eth = backrun_result.output_amount;
                let gas_cost_wei = alloy::primitives::U256::from(backrun_result.gas_used) * alloy::primitives::U256::from(20e9 as u64);
                
                if expected_profit_eth > gas_cost_wei * alloy::primitives::U256::from(2) {
                    let net_profit = expected_profit_eth - gas_cost_wei;
                    
                    return Ok(Some(Opportunity {
                        kind: OpportunityKind::Backrun,
                        trigger_tx: tx.hash,
                        path,
                        confidence: self.calculate_confidence(&backrun_result, net_profit),
                    }));
                }
            }
        }
        
        Ok(None)
    }

    async fn enqueue_opportunity(&self, opportunity: Opportunity) {
        let mut queue = self.pending_opportunities.write().await;
        
        if queue.len() >= OPPORTUNITY_BATCH_SIZE {
            queue.remove(0);
        }
        
        queue.push(opportunity);
    }

    async fn process_opportunities(&self) -> eyre::Result<()> {
        let mut queue = self.pending_opportunities.write().await;
        
        let high_confidence: Vec<_> = queue
            .drain(..)
            .filter(|opp| opp.confidence >= 0.80)
            .collect();
        
        drop(queue);

        for opportunity in high_confidence {
            match opportunity.kind {
                OpportunityKind::Backrun => {
                    info!(
                        "Executing backrun for trigger {:?}: profit={}, confidence={}",
                        opportunity.trigger_tx,
                        opportunity.path.expected_profit,
                        opportunity.confidence
                    );
                }
                OpportunityKind::BugBounty => {
                    info!(
                        "Bug bounty opportunity detected: trigger={:?}, confidence={}",
                        opportunity.trigger_tx,
                        opportunity.confidence
                    );
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn calculate_confidence(
        &self,
        sim_result: &crate::types::SimulationResult,
        net_profit: alloy::primitives::U256,
    ) -> f64 {
        let gas_efficiency = 1.0 - (sim_result.gas_used as f64 / 500000.0);
        let profit_score = f64::min(net_profit.to::<u64>() as f64 / 1e18, 1.0);
        
        (gas_efficiency * 0.4 + profit_score * 0.6).clamp(0.0, 1.0)
    }

    async fn scan_for_arbitrage(&self, _tx: &MempoolTx) {
        let weth = alloy::primitives::address!("0x4200000000000000000000000000000000000006");
        
        if let Some(path) = self.pathfinder.find_arbitrage(weth) {
            let min_profit_ratio = fixed::types::U64F64::from_num(self.config.min_profit_basis_points);
            
            if path.profit_ratio >= min_profit_ratio {
                debug!(
                    "Arbitrage opportunity detected: profit_ratio={}, hops={}",
                    path.profit_ratio, path.hops.len()
                );
                
                let opportunity = Opportunity {
                    kind: OpportunityKind::Backrun,
                    trigger_tx: _tx.hash,
                    path,
                    confidence: 0.75,
                };
                
                self.enqueue_opportunity(opportunity).await;
            }
        }
    }
}
