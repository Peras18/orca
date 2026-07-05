with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

replacements = [
    ('pub async fn execute_opportunity(&self, opportunity: Opportunity) -> Option<ExecutionReceipt> {\n        info!("[DIAG-EXEC] 1. entrou em execute_opportunity");',
     'pub async fn execute_opportunity(&self, opportunity: Opportunity) -> Option<ExecutionReceipt> {\n        let exec_id = EXEC_ID_COUNTER.fetch_add(1, Ordering::Relaxed);\n        info!("[DIAG-EXEC] id={} 1. entrou em execute_opportunity", exec_id);'),
    ('info!("[DIAG-EXEC] 2. passou validate_opportunity");',
     'info!("[DIAG-EXEC] id={} 2. passou validate_opportunity", exec_id);'),
    ('info!("[DIAG-EXEC] 3. passou kill-switch");',
     'info!("[DIAG-EXEC] id={} 3. passou kill-switch", exec_id);'),
    ('info!("[DIAG-EXEC] 4. bundle construído, a entrar em await_optimal_window");',
     'info!("[DIAG-EXEC] id={} 4. bundle construído, a entrar em await_optimal_window", exec_id);'),
    ('info!("[DIAG-EXEC] 5. await_optimal_window concluído");',
     'info!("[DIAG-EXEC] id={} 5. await_optimal_window concluído", exec_id);'),
    ('info!("[DIAG-EXEC] 6. calculate_optimal_send_time concluído");',
     'info!("[DIAG-EXEC] id={} 6. calculate_optimal_send_time concluído", exec_id);'),
    ('info!("[DIAG-EXEC] 7. wait_for_send_window concluído");',
     'info!("[DIAG-EXEC] id={} 7. wait_for_send_window concluído", exec_id);'),
]

print("=== Verificação ===")
all_ok = True
for old, new in replacements:
    count = content.count(old)
    print(f"'{old[:60]}...': {count} ocorrências")
    if count != 1:
        all_ok = False

if all_ok:
    for old, new in replacements:
        content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Todas as substituições aplicadas com sucesso.")
else:
    print("ABORTADO -- alguma contagem != 1.")
