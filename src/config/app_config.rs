//! Configuração centralizada do bot MEV
//! 
//! Este módulo gere todas as variáveis de ambiente e configurações
//! do bot através de um ficheiro .env

use alloy::primitives::{Address, U256};
use tracing::{error, info, warn};

/// Estrutura principal de configuração da aplicação
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Lista de WebSocket URLs (Failover)
    pub rpc_wss_urls: Vec<String>,
    
    /// Lista de HTTP URLs (Failover)
    pub rpc_http_urls: Vec<String>,

    /// Chave da Alchemy (se disponível)
    pub alchemy_key: Option<String>,
    
    /// Chave privada do wallet (hexadecimal com ou sem 0x)
    pub private_key: String,
    
    /// Endereço do contrato executor na Base
    pub executor_address: Option<Address>,
    
    /// Modo Shadow Hunter (simulação sem execução real)
    pub dry_run: bool,
    
    /// Habilitar state overlay para backrunning
    pub enable_backrun: bool,
    
    /// Lucro mínimo em ETH para registar oportunidades
    pub min_profit_eth: f64,
    
    /// Região do bot (para métricas)
    pub region: String,
    
    /// Comprimento máximo do caminho de arbitragem
    pub max_path_length: usize,
    
    /// Mínimo profit em basis points (0.01%)
    pub min_profit_basis_points: u32,
    
    /// Max TPS para Alchemy (limite de créditos)
    pub alchemy_max_tps: u32,
    
    /// GasCap: base fee máximo em gwei (bot entra em espera se ultrapassar)
    pub gas_cap_gwei: u64,
    
    /// Priority fee base em gwei (ajustado dinamicamente)
    pub priority_fee_base_gwei: u64,
    
    /// Modo espera quando gas está alto
    pub gas_wait_mode_enabled: bool,
}

/// Configuração de gas dinâmica
#[derive(Clone, Debug)]
pub struct GasConfig {
    /// GasCap em wei
    pub gas_cap_wei: u128,
    /// Priority fee atual
    pub current_priority_fee_wei: u128,
    /// Se o bot está em modo espera
    pub is_waiting: bool,
}

impl GasConfig {
    /// Converte gwei para wei
    pub fn gwei_to_wei(gwei: u64) -> u128 {
        (gwei as u128) * 1_000_000_000u128
    }
    
    /// Verifica se o gas atual está dentro do limite
    pub fn is_gas_acceptable(&self, base_fee_wei: u128) -> bool {
        if self.is_waiting {
            // Se estava em espera, verifica se baixou 20% abaixo do cap
            base_fee_wei < (self.gas_cap_wei * 8 / 10)
        } else {
            // Modo normal: aceita se <= gas_cap
            base_fee_wei <= self.gas_cap_wei
        }
    }
    
    /// Calcula priority fee competitivo baseado no lucro estimado
    pub fn calculate_priority_fee(&self, estimated_profit_wei: U256, urgency: UrgencyLevel) -> u128 {
        use alloy::primitives::U256;
        
        let base_priority = self.current_priority_fee_wei;
        
        // Ajustar baseado na urgência e lucro
        let multiplier = match urgency {
            UrgencyLevel::Low => 1u64,
            UrgencyLevel::Medium => 2u64,
            UrgencyLevel::High => 5u64,
            UrgencyLevel::Critical => 10u64,
        };
        
        // Se lucro é alto, pode pagar mais por prioridade
        let profit_based_boost = if estimated_profit_wei > U256::from(1_000_000_000_000_000u64) {
            // Lucro > 0.001 ETH, aumenta 50%
            base_priority / 2
        } else {
            0
        };
        
        (base_priority * multiplier as u128) + profit_based_boost
    }
}

/// Nível de urgência para ajuste de priority fee
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UrgencyLevel {
    Low,      // Oportunidade comum
    Medium,   // Boa oportunidade
    High,     // Oportunidade excelente
    Critical, // Oportunidade única/urgente
}

impl AppConfig {
    /// Carrega e valida a configuração a partir do .env
    /// 
    /// # Fail-fast
    /// Este método faz panic se variáveis obrigatórias estiverem em falta
    pub fn load() -> Self {
        // Forçar o carregamento do .env logo no início
        if let Err(e) = dotenvy::dotenv() {
            eprintln!("⚠️  AVISO: Não foi possível carregar o ficheiro .env: {}", e);
        } else {
            println!("✅ Ficheiro .env carregado com sucesso.");
        }
        
        info!("📝 Carregando configurações do .env...");
        
        // Alchemy Key (Opcional, mas recomendada)
        let alchemy_key = std::env::var("ALCHEMY_KEY").ok();
        
        // RPC List Construction
        let mut rpc_wss_urls = Vec::new();
        let mut rpc_http_urls = Vec::new();

        // 1. Se Alchemy Key existir, adicionar Alchemy como primário
        if let Some(ref key) = alchemy_key {
            rpc_wss_urls.push(format!("wss://base-mainnet.g.alchemy.com/v2/{}", key));
            rpc_http_urls.push(format!("https://base-mainnet.g.alchemy.com/v2/{}", key));
        }

        // 2. Adicionar RPC Público da Base como Fallback (Obrigatório)
        // Nota: wss://mainnet.base.org não suporta WebSocket, usando Ankr como fallback
        rpc_wss_urls.push("wss://rpc.ankr.com/base".to_string());
        rpc_http_urls.push("https://mainnet.base.org".to_string());

        // 3. Adicionar outros RPCs do .env se existirem
        if let Ok(extra_wss) = std::env::var("RPC_WSS_URLS") {
            for url in extra_wss.split(',') {
                let trimmed = url.trim();
                if !trimmed.is_empty() && !rpc_wss_urls.contains(&trimmed.to_string()) {
                    rpc_wss_urls.push(trimmed.to_string());
                }
            }
        }

        if let Ok(extra_http) = std::env::var("RPC_HTTP_URLS") {
            for url in extra_http.split(',') {
                let trimmed = url.trim();
                if !trimmed.is_empty() && !rpc_http_urls.contains(&trimmed.to_string()) {
                    rpc_http_urls.push(trimmed.to_string());
                }
            }
        }

        let private_key = Self::require_env_var("PRIVATE_KEY");
        
        // Validar formato da chave privada
        Self::validate_private_key(&private_key);
        
        // Endereço do executor (opcional mas recomendado)
        let executor_address = Self::parse_executor_address();
        
        // Configurações com valores padrão
        let dry_run = Self::parse_bool_env("DRY_RUN", true);
        let enable_backrun = Self::parse_bool_env("ENABLE_BACKRUN", false);
        let min_profit_eth = Self::parse_f64_env("MIN_PROFIT_ETH", 0.00005); // ~0.15€ para micro-arbitragens
        let region = Self::parse_string_env("REGION", "us-east-1");
        let max_path_length = Self::parse_usize_env("MAX_PATH_LENGTH", 4);
        let min_profit_basis_points = Self::parse_u32_env("MIN_PROFIT_BASIS_POINTS", 50);
        let alchemy_max_tps = Self::parse_u32_env("ALCHEMY_MAX_TPS", 100);
        
        // Configurações de gas dinâmico
        let gas_cap_gwei = Self::parse_u64_env("GAS_CAP_GWEI", 100); // 0.1 gwei = 100 Mwei
        let priority_fee_base_gwei = Self::parse_u64_env("PRIORITY_FEE_BASE_GWEI", 1); // 1 gwei
        let gas_wait_mode_enabled = Self::parse_bool_env("GAS_WAIT_MODE_ENABLED", true);
        
        let config = Self {
            rpc_wss_urls,
            rpc_http_urls,
            alchemy_key,
            private_key,
            executor_address,
            dry_run,
            enable_backrun,
            min_profit_eth,
            region,
            max_path_length,
            min_profit_basis_points,
            alchemy_max_tps,
            gas_cap_gwei,
            priority_fee_base_gwei,
            gas_wait_mode_enabled,
        };
        
        config.print_startup_summary();
        config.validate_optional();
        
        info!("✅ Configuração carregada com sucesso");
        
        config
    }
    
    /// Obtém uma variável de ambiente obrigatória
    /// Falha segura (panic) se não estiver definida
    fn require_env_var(name: &str) -> String {
        match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => {
                info!("  ✓ {}: [DEFINIDO]", name);
                value.trim().to_string()
            }
            _ => {
                error!("❌ ERRO CRÍTICO: Variável obrigatória '{}' não está definida no .env", name);
                error!("   O bot não pode iniciar sem esta configuração.");
                error!("   Por favor, defina-a no ficheiro .env e tente novamente.");
                panic!("Falta variável obrigatória: {}", name);
            }
        }
    }
    
    /// Valida o formato da chave privada (hexadecimal de 64 caracteres)
    fn validate_private_key(key: &str) {
        // Remover prefixo 0x se existir
        let clean_key = key.trim_start_matches("0x");
        
        // Verificar se é hexadecimal válido
        if clean_key.len() != 64 {
            error!("❌ ERRO: PRIVATE_KEY deve ter exatamente 64 caracteres hexadecimais");
            error!("   Comprimento atual: {} caracteres", clean_key.len());
            panic!("Formato inválido de PRIVATE_KEY");
        }
        
        if !clean_key.chars().all(|c| c.is_ascii_hexdigit()) {
            error!("❌ ERRO: PRIVATE_KEY contém caracteres não-hexadecimais");
            panic!("PRIVATE_KEY deve ser hexadecimal (0-9, a-f, A-F)");
        }
        
        // Verificar se não é uma chave óbvia/inválida
        let lower_key = clean_key.to_lowercase();
        if lower_key.chars().all(|c| c == '0' || c == 'f') {
            warn!("⚠️  AVISO: PRIVATE_KEY parece ser uma chave de teste/trivial");
        }
        
        info!("  ✓ PRIVATE_KEY: [VÁLIDO - 64 chars hex]");
    }
    
    /// Faz parse do endereço do executor
    fn parse_executor_address() -> Option<Address> {
        match std::env::var("EXECUTOR_ADDRESS") {
            Ok(addr_str) if !addr_str.trim().is_empty() => {
                let clean = addr_str.trim().to_lowercase();
                match clean.parse::<Address>() {
                    Ok(addr) => {
                        info!("  ✓ EXECUTOR_ADDRESS: {:?}", addr);
                        Some(addr)
                    }
                    Err(e) => {
                        error!("❌ ERRO: EXECUTOR_ADDRESS inválido: {}", e);
                        panic!("Endereço do executor inválido");
                    }
                }
            }
            _ => {
                warn!("  ⚠ EXECUTOR_ADDRESS: [NÃO DEFINIDO]");
                warn!("    O bot funcionará em modo Shadow Hunter apenas");
                None
            }
        }
    }
    
    /// Faz parse de variável booleana
    fn parse_bool_env(name: &str, default: bool) -> bool {
        match std::env::var(name) {
            Ok(val) => {
                let val_lower = val.trim().to_lowercase();
                let result = matches!(val_lower.as_str(), "true" | "1" | "yes" | "on");
                info!("  ✓ {}: {}", name, result);
                result
            }
            Err(_) => {
                if default {
                    info!("  ✓ {}: {} (padrão)", name, default);
                } else {
                    info!("  ✓ {}: {}", name, default);
                }
                default
            }
        }
    }
    
    /// Faz parse de variável f64
    fn parse_f64_env(name: &str, default: f64) -> f64 {
        match std::env::var(name) {
            Ok(val) => {
                match val.trim().parse::<f64>() {
                    Ok(v) if v >= 0.0 => {
                        info!("  ✓ {}: {} ETH", name, v);
                        v
                    }
                    _ => {
                        warn!("  ⚠ {}: valor inválido, usando padrão {}", name, default);
                        default
                    }
                }
            }
            Err(_) => {
                info!("  ✓ {}: {} ETH (padrão)", name, default);
                default
            }
        }
    }
    
    /// Faz parse de variável String
    fn parse_string_env(name: &str, default: &str) -> String {
        match std::env::var(name) {
            Ok(val) if !val.trim().is_empty() => {
                info!("  ✓ {}: {}", name, val.trim());
                val.trim().to_string()
            }
            _ => {
                info!("  ✓ {}: {} (padrão)", name, default);
                default.to_string()
            }
        }
    }
    
    /// Faz parse de variável usize
    fn parse_usize_env(name: &str, default: usize) -> usize {
        match std::env::var(name) {
            Ok(val) => {
                match val.trim().parse::<usize>() {
                    Ok(v) if v > 0 => {
                        info!("  ✓ {}: {}", name, v);
                        v
                    }
                    _ => {
                        warn!("  ⚠ {}: valor inválido, usando padrão {}", name, default);
                        default
                    }
                }
            }
            Err(_) => {
                info!("  ✓ {}: {} (padrão)", name, default);
                default
            }
        }
    }
    
    /// Faz parse de variável u32
    fn parse_u32_env(name: &str, default: u32) -> u32 {
        match std::env::var(name) {
            Ok(val) => {
                match val.trim().parse::<u32>() {
                    Ok(v) => {
                        info!("  ✓ {}: {}", name, v);
                        v
                    }
                    _ => {
                        warn!("  ⚠ {}: valor inválido, usando padrão {}", name, default);
                        default
                    }
                }
            }
            Err(_) => {
                info!("  ✓ {}: {} (padrão)", name, default);
                default
            }
        }
    }
    
    /// Faz parse de variável u64 (para configurações de gas)
    fn parse_u64_env(name: &str, default: u64) -> u64 {
        match std::env::var(name) {
            Ok(val) => {
                match val.trim().parse::<u64>() {
                    Ok(v) => {
                        info!("  ✓ {}: {}", name, v);
                        v
                    }
                    _ => {
                        warn!("  ⚠ {}: valor inválido, usando padrão {}", name, default);
                        default
                    }
                }
            }
            Err(_) => {
                info!("  ✓ {}: {} (padrão)", name, default);
                default
            }
        }
    }
    
    /// Validações adicionais de configuração
    fn validate_optional(&self) {
        // Verificações de consistência
        if !self.dry_run && self.executor_address.is_none() {
            error!("❌ ERRO: DRY_RUN=false mas EXECUTOR_ADDRESS não está definido");
            error!("   Não é possível executar transações sem um contrato executor");
            panic!("Configuração inválida: modo LIVE requer EXECUTOR_ADDRESS");
        }
        
        // Validar URLs WebSocket
        for url in &self.rpc_wss_urls {
            if !url.starts_with("ws://") && !url.starts_with("wss://") {
                warn!("⚠️  AVISO: A URL WebSocket {} deve começar com ws:// ou wss://", 
                      url.replace("wss://", "wss://***"));
            }
        }
    }
    
    /// Imprime resumo de inicialização no terminal
    /// Este output vai APENAS para o console (não para ficheiro de log de lucro)
    fn print_startup_summary(&self) {
        // Este println vai apenas para o terminal, não para os logs de ficheiro
        let mode = if self.dry_run { "DRY_RUN" } else { "LIVE" };
        let backrun = if self.enable_backrun { "ON" } else { "OFF" };
        let wss_count = self.rpc_wss_urls.len();
        
        println!("\n═══════════════════════════════════════════════════════════");
        println!("                    MODO DE OPERAÇÃO                        ");
        println!("═══════════════════════════════════════════════════════════");
        println!("MODO: {} | BACKRUN: {} | RPCs: {}", 
                 mode, backrun, wss_count);
        println!("ALCHEMY MAX TPS: {} (limite de créditos)", self.alchemy_max_tps);
        println!("═══════════════════════════════════════════════════════════\n");
        
        // Também logar via tracing (vai para o ficheiro)
        info!("MODO: {} | BACKRUN: {} | RPCs: {}", 
              mode, backrun, wss_count);
    }
    
    /// Verifica se estamos em modo Shadow Hunter
    pub fn is_shadow_hunter(&self) -> bool {
        self.dry_run
    }
    
    /// Verifica se backrunning está ativo
    pub fn is_backrun_enabled(&self) -> bool {
        self.enable_backrun
    }
    
    /// Retorna o endereço do executor (panics se não definido em modo LIVE)
    pub fn executor_address(&self) -> Address {
        match self.executor_address {
            Some(addr) => addr,
            None => {
                if self.dry_run {
                    // Em modo DRY_RUN, podemos usar um endereço dummy
                    Address::ZERO
                } else {
                    panic!("EXECUTOR_ADDRESS não definido");
                }
            }
        }
    }
}

/// Testes de validação
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_validate_private_key_valid() {
        // Não deve panic
        AppConfig::validate_private_key("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        AppConfig::validate_private_key("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    }
    
    #[test]
    #[should_panic]
    fn test_validate_private_key_invalid_length() {
        AppConfig::validate_private_key("0123456789abcdef"); // Muito curto
    }
    
    #[test]
    #[should_panic]
    fn test_validate_private_key_invalid_chars() {
        AppConfig::validate_private_key("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abczzz");
    }
    
    #[test]
    fn test_parse_bool_env() {
        std::env::set_var("TEST_BOOL", "true");
        assert!(AppConfig::parse_bool_env("TEST_BOOL", false));
        
        std::env::set_var("TEST_BOOL", "1");
        assert!(AppConfig::parse_bool_env("TEST_BOOL", false));
        
        std::env::set_var("TEST_BOOL", "false");
        assert!(!AppConfig::parse_bool_env("TEST_BOOL", true));
        
        std::env::remove_var("TEST_BOOL");
        assert!(AppConfig::parse_bool_env("TEST_BOOL_NONEXISTENT", true));
    }
}
