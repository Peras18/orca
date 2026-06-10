//! Script de Deploy do ApexMEVExecutor
//! Script simplificado que gera o bytecode e instruções para deploy manual

use std::path::Path;
use std::fs;
use std::process::Command;
use eyre::Result;
use tracing::{info, error};

/// Configuração de deploy
pub struct DeployConfig {
    pub chain_id: u64, // Base = 8453
}

impl Default for DeployConfig {
    fn default() -> Self {
        Self {
            chain_id: 8453,
        }
    }
}

/// Resultado do deploy (informações para referência)
#[derive(Clone, Debug)]
pub struct DeployInfo {
    pub expected_address: String,
    pub constructor_args: String,
    pub gas_estimate: u64,
    pub verification_command: String,
}

/// Deployer do contrato MEV Executor
pub struct ContractDeployer {
    config: DeployConfig,
}

impl ContractDeployer {
    pub fn new(config: DeployConfig) -> Self {
        Self { config }
    }

    /// Compila o contrato Solidity usando forge/solc
    pub fn compile_contract(&self) -> Result<String> {
        info!("Compilando contrato ApexMEVExecutor...");

        let contracts_path = Path::new("contracts");
        let main_contract = contracts_path.join("ApexMEVExecutor.sol");

        if !main_contract.exists() {
            error!("Contrato não encontrado em: {:?}", main_contract);
            return Err(eyre::eyre!("Contract file not found"));
        }

        // Usar forge build se disponível
        let output = Command::new("forge")
            .args(&["build", "--contracts", "contracts/", "--optimize", "--optimizer-runs", "1000000"])
            .current_dir(".")
            .output();

        match output {
            Ok(result) if result.status.success() => {
                info!("Compilação com Forge bem-sucedida!");
            }
            _ => {
                // Fallback: instruções manuais
                info!("Forge não encontrado. Instruções para compilação manual:");
                info!("1. Instale Foundry: curl -L https://foundry.paradigm.xyz | bash");
                info!("2. Compile: forge build --contracts contracts/ --optimize --optimizer-runs 1000000");
            }
        }

        // Verificar se o bytecode foi gerado
        let out_path = Path::new("out/ApexMEVExecutor.sol/ApexMEVExecutor.json");
        if out_path.exists() {
            let content = fs::read_to_string(out_path)?;
            info!("Bytecode gerado com sucesso!");
            info!("Arquivo: {:?}", out_path.canonicalize()?);
            return Ok(content);
        }

        // Instruções para deploy manual
        info!("═══════════════════════════════════════════════════════════════");
        info!("INSTRUÇÕES PARA DEPLOY MANUAL:");
        info!("═══════════════════════════════════════════════════════════════");
        info!("");
        info!("1. Instale Foundry (se ainda não tiver):");
        info!("   curl -L https://foundry.paradigm.xyz | bash");
        info!("   foundryup");
        info!("");
        info!("2. Compile o contrato:");
        info!("   forge build --contracts contracts/ --optimize --optimizer-runs 1000000");
        info!("");
        info!("3. Configure a chave privada:");
        info!("   export PRIVATE_KEY=0x...");
        info!("   export RPC_URL=https://mainnet.base.org");
        info!("");
        info!("4. Faça o deploy:");
        info!("   forge create contracts/ApexMEVExecutor.sol:ApexMEVExecutor \\");
        info!("     --rpc-url $RPC_URL \\");
        info!("     --private-key $PRIVATE_KEY \\");
        info!("     --optimize \\");
        info!("     --optimizer-runs 1000000 \\");
        info!("     --chain 8453");
        info!("");
        info!("5. Salve o endereço em .env:");
        info!("   echo \"EXECUTOR_ADDRESS=<ENDERECO>\" >> .env");
        info!("");
        info!("═══════════════════════════════════════════════════════════════");

        Err(eyre::eyre!("Bytecode not found. Follow instructions above."))
    }

    /// Gera informações para deploy
    pub fn generate_deploy_info(&self) -> DeployInfo {
        let expected_addr = "0x..."; // Será determinado após deploy
        
        DeployInfo {
            expected_address: expected_addr.to_string(),
            constructor_args: "None (constructor sem argumentos)".to_string(),
            gas_estimate: 2_500_000,
            verification_command: format!(
                "forge verify-contract \"{}\" ApexMEVExecutor --chain-id {} --verifier-url https://api.basescan.org/api",
                expected_addr, self.config.chain_id
            ),
        }
    }

    /// Salva endereço no .env
    pub fn save_to_env(&self, address: &str) -> Result<()> {
        let env_path = Path::new(".env");
        
        let env_content = format!(
            "# ApexMEVExecutor Deploy\nEXECUTOR_ADDRESS={}\nCHAIN_ID={}\n",
            address,
            self.config.chain_id
        );
        
        // Se .env existe, preservar outras variáveis
        let mut existing_content = String::new();
        if env_path.exists() {
            existing_content = fs::read_to_string(env_path)?;
            
            // Remover linhas antigas do executor
            let lines: Vec<&str> = existing_content.lines().collect();
            let filtered: Vec<&str> = lines.iter()
                .filter(|line| !line.starts_with("EXECUTOR_ADDRESS"))
                .copied()
                .collect();
            existing_content = filtered.join("\n");
        }
        
        let final_content = format!("{}\n{}", existing_content, env_content);
        fs::write(env_path, final_content)?;
        
        info!("Endereço salvo em .env: {}", address);
        
        Ok(())
    }

    /// Verifica o contrato no explorador
    pub fn verify_instructions(&self, address: &str) {
        info!("═══════════════════════════════════════════════════════════════");
        info!("VERIFICAÇÃO NO BASESCAN:");
        info!("═══════════════════════════════════════════════════════════════");
        info!("");
        info!("1. Acesse: https://basescan.org/address/{}", address);
        info!("2. Clique: Contract → Verify and Publish");
        info!("3. Compiler Type: Solidity (Standard-Json-Input)");
        info!("4. Compiler Version: 0.8.20");
        info!("5. Optimization: Yes, with 1,000,000 runs");
        info!("6. EVM Version: Paris");
        info!("7. Upload: out/ApexMEVExecutor.sol/ApexMEVExecutor.json");
        info!("");
        info!("OU use o comando forge:");
        info!("   forge verify-contract \"{}\" ApexMEVExecutor --chain-id 8453", address);
        info!("");
        info!("═══════════════════════════════════════════════════════════════");
    }
}

/// Função principal de deploy
pub async fn deploy_executor() -> Result<()> {
    let config = DeployConfig::default();
    let deployer = ContractDeployer::new(config);
    
    // Tentar compilar
    match deployer.compile_contract() {
        Ok(_) => {
            info!("✅ Contrato compilado com sucesso!");
        }
        Err(e) => {
            info!("⚠️  Compilação automática falhou: {}", e);
        }
    }
    
    // Gerar info de deploy
    let info = deployer.generate_deploy_info();
    info!("");
    info!("Estimativa de gas: {} gas", info.gas_estimate);
    info!("Custo estimado: ~{} ETH (a 1 gwei)", info.gas_estimate as f64 / 1e9);
    
    Ok(())
}
