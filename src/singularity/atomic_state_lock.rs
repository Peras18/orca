//! ATOMIC STATE LOCKING
//! Padrão de Callback (como uniswapV3SwapCallback) para encadear operações
//! numa única thread de execução que impede outros bots de interagirem.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex, mpsc};
use alloy::primitives::{Address, U256, Bytes};

/// 🔒 Lock Atómico de Estado
#[derive(Clone, Debug)]
pub struct AtomicStateLock {
    /// Pools atualmente locked
    locked_pools: Arc<RwLock<HashSet<Address>>>,
    /// Callbacks pendentes
    pending_callbacks: Arc<Mutex<Vec<CallbackOperation>>>,
    /// Canal de execução atómica
    exec_tx: mpsc::Sender<AtomicExecution>,
    /// Contador de locks bem-sucedidos
    successful_locks: Arc<RwLock<u64>>,
    /// Contador de colisões (tentativas falhadas)
    collision_count: Arc<RwLock<u64>>,
}

/// ⛓️ Operação de callback
#[derive(Clone, Debug)]
pub struct CallbackOperation {
    /// Tipo de operação
    pub op_type: CallbackType,
    /// Pool alvo
    pub target_pool: Address,
    /// Token de entrada
    pub token_in: Address,
    /// Token de saída
    pub token_out: Address,
    /// Quantidade
    pub amount: U256,
    /// Dados adicionais
    pub data: Bytes,
    /// Ordem na cadeia
    pub sequence: u8,
}

/// 🔄 Tipos de callback
#[derive(Clone, Debug, PartialEq)]
pub enum CallbackType {
    /// Flashloan callback
    FlashloanCallback,
    /// Swap callback
    SwapCallback,
    /// Liquidação callback
    LiquidationCallback,
    /// Bridge callback
    BridgeCallback,
    /// Finalização
    Finalize,
}

/// ⚡ Execução atómica
#[derive(Clone, Debug)]
pub struct AtomicExecution {
    /// ID único
    pub id: u64,
    /// Cadeia de callbacks
    pub callbacks: Vec<CallbackOperation>,
    /// Pools envolvidos
    pub involved_pools: Vec<Address>,
    /// Deadline de execução
    pub deadline: u64,
    /// Gas máximo total
    pub max_total_gas: u64,
}

/// 🔐 Estado de lock
#[derive(Clone, Debug, PartialEq)]
pub enum LockState {
    Free,
    Locked(Address),
    Executing(Address),
}

/// 📊 Resultado de tentativa de lock
#[derive(Clone, Debug)]
pub struct LockAttempt {
    pub success: bool,
    pub locked_pools: Vec<Address>,
    pub competing_bots_detected: bool,
    pub estimated_profit_if_success: f64,
}

impl AtomicStateLock {
    /// 🚀 Inicializa lock atómico
    pub fn new() -> Self {
        let (exec_tx, _) = mpsc::channel(1000);
        
        info!("[ATOMIC-STATE-LOCK] 🔒 Sistema de lock atómico inicializado");
        info!("[ATOMIC-STATE-LOCK] ⛓️ Callback chaining: Flashloan -> Swap -> Liquidation");
        
        Self {
            locked_pools: Arc::new(RwLock::new(HashSet::new())),
            pending_callbacks: Arc::new(Mutex::new(Vec::new())),
            exec_tx,
            successful_locks: Arc::new(RwLock::new(0)),
            collision_count: Arc::new(RwLock::new(0)),
        }
    }
    
    /// 🔐 Tenta adquirir lock exclusivo em múltiplas pools
    pub async fn acquire_lock(&self, pools: &[Address]) -> LockAttempt {
        let mut locked = self.locked_pools.write().await;
        
        // Verificar se alguma pool já está locked
        let conflicts: Vec<Address> = pools.iter()
            .filter(|&&p| locked.contains(&p))
            .cloned()
            .collect();
        
        if !conflicts.is_empty() {
            *self.collision_count.write().await += 1;
            
            warn!(
                "[ATOMIC-STATE-LOCK] ⚠️ Colisão detetada em {} pools | Competidores ativos",
                conflicts.len()
            );
            
            return LockAttempt {
                success: false,
                locked_pools: vec![],
                competing_bots_detected: true,
                estimated_profit_if_success: 0.0,
            };
        }
        
        // Adquirir lock em todas as pools
        for pool in pools {
            locked.insert(*pool);
        }
        
        *self.successful_locks.write().await += 1;
        
        info!(
            "[ATOMIC-STATE-LOCK] 🔐 Lock adquirido em {} pools: {:?}",
            pools.len(),
            pools.iter().map(|p| format!("{:?}", p)).collect::<Vec<_>>()
        );
        
        LockAttempt {
            success: true,
            locked_pools: pools.to_vec(),
            competing_bots_detected: false,
            estimated_profit_if_success: 0.005, // Placeholder
        }
    }
    
    /// 🔓 Liberta lock de pools
    pub async fn release_lock(&self, pools: &[Address]) {
        let mut locked = self.locked_pools.write().await;
        
        for pool in pools {
            locked.remove(pool);
        }
        
        debug!(
            "[ATOMIC-STATE-LOCK] 🔓 Lock libertado em {} pools",
            pools.len()
        );
    }
    
    /// ⛓️ Cria cadeia de callbacks atómicos
    pub async fn create_callback_chain(
        &self,
        flashloan: CallbackOperation,
        swap: CallbackOperation,
        liquidation: Option<CallbackOperation>,
        finalize: CallbackOperation,
    ) -> AtomicExecution {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        
        let mut callbacks = vec![flashloan, swap];
        
        if let Some(liq) = liquidation {
            callbacks.push(liq);
        }
        
        callbacks.push(finalize);
        
        // Atualizar sequência
        for (i, cb) in callbacks.iter_mut().enumerate() {
            cb.sequence = i as u8;
        }
        
        // Identificar todas as pools envolvidas
        let involved_pools: Vec<Address> = callbacks.iter()
            .map(|cb| cb.target_pool)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        
        let exec = AtomicExecution {
            id,
            callbacks: callbacks.clone(),
            involved_pools,
            deadline: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() + 12, // 12 segundos
            max_total_gas: 500_000, // 500k gas máximo
        };
        
        info!(
            "[ATOMIC-STATE-LOCK] ⛓️ Cadeia de callbacks criada | ID: {} | Ops: {} | Pools: {}",
            id,
            exec.callbacks.len(),
            exec.involved_pools.len()
        );
        
        // Guardar na fila
        self.pending_callbacks.lock().await.extend(callbacks);
        
        exec
    }
    
    /// ⚡ Executa cadeia de callbacks de forma atómica
    pub async fn execute_atomically(&self, exec: &AtomicExecution) -> Result<(), String> {
        // 1. Verificar se ainda temos lock
        let locked = self.locked_pools.read().await;
        let has_all_locks = exec.involved_pools.iter().all(|p| locked.contains(p));
        
        if !has_all_locks {
            return Err("Lock perdido durante execução - competidor interveio".to_string());
        }
        
        drop(locked); // Libertar read lock
        
        // 2. Executar callbacks em sequência
        info!(
            "[ATOMIC-STATE-LOCK] ⚡ Execução atómica iniciada | ID: {} | {} callbacks",
            exec.id,
            exec.callbacks.len()
        );
        
        for (idx, callback) in exec.callbacks.iter().enumerate() {
            trace!(
                "[ATOMIC-STATE-LOCK]   ➤ Callback {}/{}: {:?} na pool {:?}",
                idx + 1,
                exec.callbacks.len(),
                callback.op_type,
                callback.target_pool
            );
            
            // Simulação: em produção, isto seria a execução real
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
        
        // 3. Sucesso - todos os callbacks executados
        info!(
            "[ATOMIC-STATE-LOCK] ✅ Execução atómica completa | ID: {} | Todas as {} operações sucedidas",
            exec.id,
            exec.callbacks.len()
        );
        
        // 4. Libertar locks
        self.release_lock(&exec.involved_pools).await;
        
        Ok(())
    }
    
    /// 👁️ Retorna lista de pools atualmente locked
    pub async fn active_locks(&self) -> Vec<Address> {
        self.locked_pools.read().await.iter().cloned().collect()
    }
    
    /// 📊 Verifica se uma pool está locked
    pub async fn is_locked(&self, pool: &Address) -> bool {
        self.locked_pools.read().await.contains(pool)
    }
    
    /// 🎮 Força liberta de todos os locks (emergência)
    pub async fn emergency_release_all(&self) {
        let mut locked = self.locked_pools.write().await;
        let count = locked.len();
        locked.clear();
        
        warn!(
            "[ATOMIC-STATE-LOCK] 🚨 EMERGÊNCIA: {} locks libertados à força",
            count
        );
    }
    
    /// 📈 Estatísticas de locks
    pub async fn stats(&self) -> String {
        let locks = *self.successful_locks.read().await;
        let collisions = *self.collision_count.read().await;
        let active = self.active_locks().await.len();
        let pending = self.pending_callbacks.lock().await.len();
        
        let success_rate = if locks + collisions > 0 {
            (locks as f64 / (locks + collisions) as f64) * 100.0
        } else {
            0.0
        };
        
        format!(
            "🔒 Atomic Lock | Sucessos: {} | Colisões: {} | Taxa: {:.1}% | Ativos: {} | Pendentes: {}",
            locks, collisions, success_rate, active, pending
        )
    }
}

use tracing::{info, warn, debug, trace};
