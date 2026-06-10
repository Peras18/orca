//! CLI Tool para deploy do ApexMEVExecutor
//! 
//! Uso:
//!   cargo run --bin deploy
//! 
//! Este script gera instruções para compilação e deploy manual.
//! O deploy real deve ser feito via Foundry/Forge para máxima compatibilidade.

use orca_mev::deployer::deploy_executor;
use tracing::info;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Inicializar logging
    tracing_subscriber::fmt()
        .with_env_filter("orca_mev=info,deploy=info")
        .with_target(true)
        .with_thread_ids(true)
        .init();
    
    info!("╔══════════════════════════════════════════════════════════╗");
    info!("║   ApexMEV Executor Deploy Tool                           ║");
    info!("║   Base Network - Ultra-Low Latency Contract              ║");
    info!("╚══════════════════════════════════════════════════════════╝");
    
    // Executar deploy (gera instruções)
    match deploy_executor().await {
        Ok(()) => {
            info!("✅ Setup concluído!");
            info!("");
            info!("Próximos passos:");
            info!("  1. Siga as instruções acima para compilar o contrato");
            info!("  2. Faça o deploy usando Forge/Foundry");
            info!("  3. Salve o endereço do contrato em .env");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠️  Nota: {}", e);
            info!("");
            info!("Isso é esperado se o Foundry não estiver instalado.");
            info!("Siga as instruções manuais acima.");
            Ok(())
        }
    }
}
