//! ATOMIC MULTI-ACTION
//! Contrato de execução agregando: [Flashloan] -> [Start Swap] -> [Callback: Liquidate/Arb] -> [Complete Swap] -> [Repay]
//!
//! Tudo numa thread de execução atómica.

use alloy::primitives::{Address, U256};
use std::collections::VecDeque;

/// ⛓️ Executor Multi-Ação Atómico
#[derive(Clone, Debug)]
pub struct AtomicMultiAction {
    /// Ações pendentes
    pending_actions: VecDeque<GhostAction>,
    /// Execuções completadas
    completed_executions: Vec<GhostExecution>,
    /// Contador de execuções
    execution_count: u64,
    /// Gas usado acumulado
    total_gas_used: u64,
    /// Valor total capturado
    total_value_captured: f64,
}

/// 👻 Ação Fantasma
#[derive(Clone, Debug)]
pub enum GhostAction {
    /// Flashloan inicial
    Flashloan {
        token: Address,
        amount: U256,
    },
    /// Iniciar swap (dispara callback)
    StartSwap {
        pool: Address,
        params: super::GhostSwapParams,
    },
    /// Hijack do callback
    CallbackHijack {
        target_protocol: super::TargetProtocol,
        action: String,
    },
    /// Completar swap
    CompleteSwap {
        pool: Address,
    },
    /// Repay do flashloan
    RepayFlashloan {
        token: Address,
        amount: U256,
    },
}

/// 💀 Execução Fantasma Completa
#[derive(Clone, Debug)]
pub struct GhostExecution {
    /// ID da execução
    pub id: u64,
    /// Ações executadas
    pub actions: Vec<ExecutedAction>,
    /// Se toda a cadeia foi bem-sucedida
    pub all_success: bool,
    /// Gas total usado
    pub total_gas: u64,
    /// Valor total capturado (ETH)
    pub value_captured: f64,
    /// Protocolos tocados
    pub protocols_touched: Vec<super::TargetProtocol>,
    /// Timestamp
    pub timestamp: u64,
}

/// ✅ Ação Executada
#[derive(Clone, Debug)]
pub struct ExecutedAction {
    /// Ação original
    pub action: GhostAction,
    /// Sucesso
    pub success: bool,
    /// Gas usado
    pub gas_used: u64,
    /// Valor gerado (se aplicável)
    pub value_generated: f64,
    /// Logs gerados
    pub logs: Vec<String>,
}

/// 🎭 Estado da execução
#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionState {
    /// Aguardando início
    Idle,
    /// Flashloan ativo
    FlashloanActive,
    /// Swap em progresso (dentro do callback)
    SwapInProgress,
    /// Callback hijack executando
    CallbackExecuting,
    /// Swap completando
    SwapCompleting,
    /// Repay pendente
    RepayPending,
    /// Completo
    Complete,
    /// Falhou
    Failed(String),
}

impl AtomicMultiAction {
    /// 🚀 Inicializa executor
    pub fn new() -> Self {
        info!(
            "[ATOMIC-MULTI-ACTION] ⛓️ Executor inicializado - Cadeia atómica pronta"
        );
        
        Self {
            pending_actions: VecDeque::new(),
            completed_executions: Vec::new(),
            execution_count: 0,
            total_gas_used: 0,
            total_value_captured: 0.0,
        }
    }
    
    /// ⚡ Executa cadeia de ações fantasma
    pub async fn execute_ghost_chain(
        &mut self,
        actions: Vec<GhostAction>,
    ) -> Option<GhostExecution> {
        self.execution_count += 1;
        let id = self.execution_count;
        
        info!(
            "[ATOMIC-MULTI-ACTION] ⚡ EXECUÇÃO FANTASMA #{} iniciada | {} ações",
            id,
            actions.len()
        );
        
        let mut executed_actions = Vec::new();
        let mut total_gas = 0u64;
        let mut total_value = 0.0f64;
        let mut protocols_touched = Vec::new();
        let mut all_success = true;
        let mut current_state = ExecutionState::Idle;
        
        for (idx, action) in actions.iter().enumerate() {
            info!(
                "[ATOMIC-MULTI-ACTION]   ➤ [{}/{}] Executando: {:?}",
                idx + 1,
                actions.len(),
                action_name(&action)
            );
            
            // Executar ação
            let result = self.execute_single_action(&action, &mut current_state).await;
            
            let executed = ExecutedAction {
                action: action.clone(),
                success: result.success,
                gas_used: result.gas_used,
                value_generated: result.value,
                logs: result.logs,
            };
            
            total_gas += result.gas_used;
            total_value += result.value;
            
            if !result.success {
                all_success = false;
                error!(
                    "[ATOMIC-MULTI-ACTION]     ❌ FALHA em {}: {}",
                    action_name(&action),
                    result.error.unwrap_or_default()
                );
                
                // Se flashloan falhou, temos de reverter tudo
                if matches!(action, GhostAction::Flashloan { .. }) {
                    error!(
                        "[ATOMIC-MULTI-ACTION]     🔥 FALHA CRÍTICA: Flashloan falhou - revertendo"
                    );
                    break;
                }
            } else {
                // Registrar protocolo tocado
                if let Some(protocol) = extract_protocol(&action) {
                    if !protocols_touched.contains(&protocol) {
                        protocols_touched.push(protocol);
                    }
                }
                
                info!(
                    "[ATOMIC-MULTI-ACTION]     ✅ SUCESSO | Gas: {} | Valor: {} ETH",
                    result.gas_used,
                    result.value
                );
            }
            
            executed_actions.push(executed);
        }
        
        // Guardar info para logs antes de mover
        let protocols_count = protocols_touched.len();
        let protocols_for_log = protocols_touched.clone();
        
        // Criar execução
        let execution = GhostExecution {
            id,
            actions: executed_actions,
            all_success,
            total_gas,
            value_captured: total_value,
            protocols_touched,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        
        // Atualizar estatísticas
        self.total_gas_used += total_gas;
        self.total_value_captured += total_value;
        self.completed_executions.push(execution.clone());
        
        // Log [GHOST-STATE] - Capturamos valor de múltiplos protocolos
        if protocols_count >= 2 && all_success {
            info!(
                "[GHOST-STATE] 👻 FANTASMA EXECUTADO | Execução #{} | {} protocolos | {} ETH | {} gas",
                id,
                protocols_count,
                total_value,
                total_gas
            );
            
            for protocol in &protocols_for_log {
                info!(
                    "[GHOST-STATE]   🎯 Protocolo tocado: {:?}",
                    protocol
                );
            }
        }
        
        info!(
            "[ATOMIC-MULTI-ACTION] ⚡ EXECUÇÃO #{} completa | Sucesso: {} | Valor: {} ETH | Gas: {}",
            id,
            all_success,
            total_value,
            total_gas
        );
        
        Some(execution)
    }
    
    /// 🎭 Executa ação individual
    async fn execute_single_action(
        &self,
        action: &GhostAction,
        state: &mut ExecutionState,
    ) -> ActionResult {
        match action {
            GhostAction::Flashloan { token, amount } => {
                *state = ExecutionState::FlashloanActive;
                
                // Simulação: pedir flashloan
                info!(
                    "[ATOMIC-MULTI-ACTION]     💰 Flashloan solicitado | Token: {:?} | Amount: {}",
                    token, amount
                );
                
                ActionResult {
                    success: true,
                    gas_used: 50000,
                    value: 0.0,
                    logs: vec![format!("Flashloan {:?} {}", token, amount)],
                    error: None,
                }
            }
            GhostAction::StartSwap { pool, params } => {
                *state = ExecutionState::SwapInProgress;
                
                info!(
                    "[ATOMIC-MULTI-ACTION]     🔄 Swap iniciado | Pool: {:?} | Amount: {}",
                    pool, params.amount_in
                );
                
                ActionResult {
                    success: true,
                    gas_used: 80000,
                    value: 0.0,
                    logs: vec![format!("StartSwap {:?}", pool)],
                    error: None,
                }
            }
            GhostAction::CallbackHijack { target_protocol, action } => {
                *state = ExecutionState::CallbackExecuting;
                
                info!(
                    "[ATOMIC-MULTI-ACTION]     💀 CALLBACK HIJACK | Protocolo: {:?} | Ação: {}",
                    target_protocol, action
                );
                
                // Simular execução dentro do callback
                let profit = 0.015; // 0.015 ETH de lucro
                
                ActionResult {
                    success: true,
                    gas_used: 120000,
                    value: profit,
                    logs: vec![
                        format!("Hijack {:?} profit: {}", target_protocol, profit),
                        "[GHOST-STATE] Valor capturado em callback".to_string(),
                    ],
                    error: None,
                }
            }
            GhostAction::CompleteSwap { pool } => {
                *state = ExecutionState::SwapCompleting;
                
                info!(
                    "[ATOMIC-MULTI-ACTION]     ✅ Swap completando | Pool: {:?}",
                    pool
                );
                
                ActionResult {
                    success: true,
                    gas_used: 60000,
                    value: 0.0,
                    logs: vec![format!("CompleteSwap {:?}", pool)],
                    error: None,
                }
            }
            GhostAction::RepayFlashloan { token, amount } => {
                *state = ExecutionState::RepayPending;
                
                info!(
                    "[ATOMIC-MULTI-ACTION]     💸 Repay flashloan | Token: {:?} | Amount: {}",
                    token, amount
                );
                
                ActionResult {
                    success: true,
                    gas_used: 40000,
                    value: 0.0,
                    logs: vec![format!("Repay {:?} {}", token, amount)],
                    error: None,
                }
            }
        }
    }
    
    /// 📊 Estatísticas
    pub fn stats(&self) -> String {
        format!(
            "⛓️ Atomic Multi-Action | Execuções: {} | Gas total: {} | Valor: {} ETH | Média: {} ETH/exec",
            self.execution_count,
            self.total_gas_used,
            self.total_value_captured,
            if self.execution_count > 0 { self.total_value_captured / self.execution_count as f64 } else { 0.0 }
        )
    }
}

/// 📝 Resultado de execução de ação
struct ActionResult {
    success: bool,
    gas_used: u64,
    value: f64,
    logs: Vec<String>,
    error: Option<String>,
}

/// 🏷️ Extrai nome da ação
fn action_name(action: &GhostAction) -> String {
    match action {
        GhostAction::Flashloan { .. } => "Flashloan".to_string(),
        GhostAction::StartSwap { .. } => "StartSwap".to_string(),
        GhostAction::CallbackHijack { .. } => "CallbackHijack".to_string(),
        GhostAction::CompleteSwap { .. } => "CompleteSwap".to_string(),
        GhostAction::RepayFlashloan { .. } => "RepayFlashloan".to_string(),
    }
}

/// 🎯 Extrai protocolo da ação
fn extract_protocol(action: &GhostAction) -> Option<super::TargetProtocol> {
    match action {
        GhostAction::CallbackHijack { target_protocol, .. } => Some(target_protocol.clone()),
        GhostAction::Flashloan { .. } => None,
        GhostAction::StartSwap { .. } => None,
        GhostAction::CompleteSwap { .. } => None,
        GhostAction::RepayFlashloan { .. } => None,
    }
}

use tracing::{info, error};
