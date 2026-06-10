//! STATIC ANALYSIS OF BYTECODE
//! Analisa bytecode de novos tokens para identificar Mint/Tax escondidos
//! 
//! Deteta funções que simuladores normais não apanham

use alloy::primitives::{Address, Bytes, FixedBytes};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 🔍 Analisador de Bytecode
#[derive(Clone, Debug)]
pub struct BytecodeAnalyzer {
    /// Padrões perigosos conhecidos
    pub danger_patterns: Arc<RwLock<HashMap<String, BytePattern>>>,
    /// Tokens já analisados
    pub analyzed_tokens: Arc<RwLock<HashMap<Address, TokenRiskProfile>>>,
    /// Contador de análises
    analyses_count: Arc<RwLock<u64>>,
    /// Total de ameaças detetadas
    threats_found: Arc<RwLock<u64>>,
}

/// 🎭 Padrão de bytecode
#[derive(Clone, Debug)]
pub struct BytePattern {
    /// Nome do padrão
    pub name: String,
    /// Sequência de bytes (hex)
    pub bytecode_sequence: Vec<u8>,
    /// Descrição da ameaça
    pub description: String,
    /// Severidade (1-10)
    pub severity: u8,
    /// Tipo de ameaça
    pub threat_type: ThreatType,
}

/// ⚠️ Tipos de ameaça
#[derive(Clone, Debug, PartialEq)]
pub enum ThreatType {
    /// Função de mint escondida
    HiddenMint,
    /// Taxa de transferência escondida
    HiddenTransferTax,
    /// Blacklist de endereços
    BlacklistFunction,
    /// Pause/Freeze de transfers
    PausableContract,
    /// Self-destruct
    Suicidal,
    /// Proxy upgrade malicioso
    MaliciousUpgrade,
    /// Outras
    Other,
}

/// 🛡️ Perfil de risco de token
#[derive(Clone, Debug)]
pub struct TokenRiskProfile {
    /// Endereço do token
    pub address: Address,
    /// Nome do token (se conhecido)
    pub name: String,
    /// Símbolo
    pub symbol: String,
    /// Score de risco (0-100, maior = mais arriscado)
    pub risk_score: u8,
    /// Ameaças detetadas
    pub threats: Vec<DetectedThreat>,
    /// Hash do bytecode analisado
    pub bytecode_hash: String,
    /// Tamanho do bytecode
    pub bytecode_size: usize,
    /// Timestamp de análise
    pub analyzed_at: u64,
    /// Recomendação
    pub recommendation: RiskRecommendation,
}

/// 🚨 Ameaça detetada
#[derive(Clone, Debug)]
pub struct DetectedThreat {
    /// Tipo de ameaça
    pub threat_type: ThreatType,
    /// Padrão encontrado
    pub pattern_name: String,
    /// Posição no bytecode
    pub position: usize,
    /// Severidade
    pub severity: u8,
    /// Código hex encontrado
    pub hex_snippet: String,
}

/// 💡 Recomendação de risco
#[derive(Clone, Debug, PartialEq)]
pub enum RiskRecommendation {
    /// Seguro para trading
    Safe,
    /// Cautela recomendada
    Caution,
    /// Alto risco
    HighRisk,
    /// Não interagir
    Avoid,
}

/// 🔎 Função escondida encontrada
#[derive(Clone, Debug)]
pub struct HiddenFunction {
    /// Selector da função (4 bytes)
    pub selector: FixedBytes<4>,
    /// Nome inferido
    pub inferred_name: String,
    /// Tipo
    pub func_type: FuncType,
    /// Se é perigosa
    pub is_dangerous: bool,
}

/// ⚙️ Tipo de função
#[derive(Clone, Debug, PartialEq)]
pub enum FuncType {
    Mint,
    Burn,
    Transfer,
    Approve,
    Blacklist,
    Pause,
    Upgrade,
    Unknown,
}

impl BytecodeAnalyzer {
    /// 🚀 Inicializa analisador
    pub fn new() -> Self {
        let mut patterns = HashMap::new();
        
        // Padrão 1: Mint escondido (sstore + emit Transfer para endereço zero)
        patterns.insert(
            "hidden_mint".to_string(),
            BytePattern {
                name: "Hidden Mint Function".to_string(),
                bytecode_sequence: hex::decode("60405160e01b8082526020819052604081018290526060820152608081019190915260a082015260c082015260e082015261010082015261012082015261014090910155").unwrap_or_default(),
                description: "Função que minta tokens sem evento Mint visível".to_string(),
                severity: 9,
                threat_type: ThreatType::HiddenMint,
            },
        );
        
        // Padrão 2: Tax de transfer escondida
        patterns.insert(
            "hidden_tax".to_string(),
            BytePattern {
                name: "Hidden Transfer Tax".to_string(),
                bytecode_sequence: hex::decode("600060003560e01c8063a9059cbb14602f5763a9059cbb14602957005b6020601f565b005b603a565b005b6060565b005b6080565b005b60a0565b005b60c0565b005b60e0565b005b610100565b005b").unwrap_or_default(),
                description: "Taxa retida nas transfers sem ser visível no ABI".to_string(),
                severity: 7,
                threat_type: ThreatType::HiddenTransferTax,
            },
        );
        
        // Padrão 3: Blacklist
        patterns.insert(
            "blacklist".to_string(),
            BytePattern {
                name: "Address Blacklist".to_string(),
                bytecode_sequence: hex::decode("73ff00000000000000000000000000000000000000007f0100000000000000000000000000000000000000000000000000000000000000600054").unwrap_or_default(),
                description: "Mecanismo de blacklist que pode bloquear transfers".to_string(),
                severity: 8,
                threat_type: ThreatType::BlacklistFunction,
            },
        );
        
        // Padrão 4: Self-destruct
        patterns.insert(
            "suicidal".to_string(),
            BytePattern {
                name: "Self-Destruct Capability".to_string(),
                bytecode_sequence: hex::decode("730000000000000000000000000000000000000000ff").unwrap_or_default(),
                description: "Contrato pode autodestruir-se".to_string(),
                severity: 10,
                threat_type: ThreatType::Suicidal,
            },
        );
        
        info!("[BYTECODE-ANALYZER] 🔍 {} padrões de ameaça carregados", patterns.len());
        
        Self {
            danger_patterns: Arc::new(RwLock::new(patterns)),
            analyzed_tokens: Arc::new(RwLock::new(HashMap::new())),
            analyses_count: Arc::new(RwLock::new(0)),
            threats_found: Arc::new(RwLock::new(0)),
        }
    }
    
    /// 🔎 Analisa bytecode de token
    pub async fn analyze_token(&self, address: Address, bytecode: Bytes, name: &str, symbol: &str) -> TokenRiskProfile {
        *self.analyses_count.write().await += 1;
        
        let mut threats = Vec::new();
        let mut risk_score: u8 = 0;
        
        // Verificar cada padrão conhecido
        let patterns = self.danger_patterns.read().await;
        for (_pattern_name, pattern) in patterns.iter() {
            if let Some(pos) = find_pattern(&bytecode, &pattern.bytecode_sequence) {
                let hex_snippet = hex::encode(&bytecode[pos..(pos + 20).min(bytecode.len())]);
                
                threats.push(DetectedThreat {
                    threat_type: pattern.threat_type.clone(),
                    pattern_name: pattern.name.clone(),
                    position: pos,
                    severity: pattern.severity,
                    hex_snippet,
                });
                
                risk_score = risk_score.saturating_add(pattern.severity / 2);
                *self.threats_found.write().await += 1;
                
                warn!(
                    "[BYTECODE-ANALYZER] 🚨 AMEAÇA em {}: {} | Pos: {} | Sev: {}",
                    symbol,
                    pattern.name,
                    pos,
                    pattern.severity
                );
            }
        }
        
        // Analisar funções escondidas
        let hidden_funcs = self.extract_hidden_functions(&bytecode);
        for func in &hidden_funcs {
            if func.is_dangerous {
                threats.push(DetectedThreat {
                    threat_type: match func.func_type {
                        FuncType::Mint => ThreatType::HiddenMint,
                        FuncType::Blacklist => ThreatType::BlacklistFunction,
                        _ => ThreatType::Other,
                    },
                    pattern_name: format!("Hidden {:?} Function", func.func_type),
                    position: 0,
                    severity: 8,
                    hex_snippet: hex::encode(func.selector.as_slice()),
                });
                
                risk_score = risk_score.saturating_add(4);
            }
        }
        
        // Determinar recomendação
        let recommendation = if risk_score >= 20 {
            RiskRecommendation::Avoid
        } else if risk_score >= 10 {
            RiskRecommendation::HighRisk
        } else if risk_score >= 5 {
            RiskRecommendation::Caution
        } else {
            RiskRecommendation::Safe
        };
        
        // Criar hash do bytecode
        let bytecode_hash = format!("{:x}", calculate_hash(&bytecode));
        
        let profile = TokenRiskProfile {
            address,
            name: name.to_string(),
            symbol: symbol.to_string(),
            risk_score,
            threats: threats.clone(),
            bytecode_hash,
            bytecode_size: bytecode.len(),
            analyzed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            recommendation: recommendation.clone(),
        };
        
        // Guardar resultado
        self.analyzed_tokens.write().await.insert(address, profile.clone());
        
        info!(
            "[BYTECODE-ANALYZER] ✅ {} analisado | Risk: {}/100 | Rec: {:?} | Ameaças: {}",
            symbol,
            risk_score,
            recommendation,
            threats.len()
        );
        
        profile
    }
    
    /// 🔧 Extrai funções escondidas do bytecode
    fn extract_hidden_functions(&self, bytecode: &Bytes) -> Vec<HiddenFunction> {
        let mut functions = Vec::new();
        
        // Procurar por PUSH4 seguido de CALL (padrão de selector)
        for i in 0..bytecode.len().saturating_sub(5) {
            if bytecode[i] == 0x63 { // PUSH4
                let selector = FixedBytes::new([
                    bytecode[i + 1],
                    bytecode[i + 2],
                    bytecode[i + 3],
                    bytecode[i + 4],
                ]);
                
                // Verificar se é uma função perigosa conhecida
                let (name, func_type, dangerous) = self.classify_selector(selector);
                
                functions.push(HiddenFunction {
                    selector,
                    inferred_name: name,
                    func_type,
                    is_dangerous: dangerous,
                });
            }
        }
        
        functions
    }
    
    /// 🏷️ Classifica um selector de função
    fn classify_selector(&self, selector: FixedBytes<4>) -> (String, FuncType, bool) {
        let hex_sel = hex::encode(selector.as_slice());
        
        match hex_sel.as_str() {
            "40c10f19" => ("mint(address,uint256)".to_string(), FuncType::Mint, true),
            "9dc29fac" => ("burn(address,uint256)".to_string(), FuncType::Burn, true),
            "a9059cbb" => ("transfer(address,uint256)".to_string(), FuncType::Transfer, false),
            "095ea7b3" => ("approve(address,uint256)".to_string(), FuncType::Approve, false),
            "f9f51401" => ("blacklist(address)".to_string(), FuncType::Blacklist, true),
            "8456cb59" => ("pause()".to_string(), FuncType::Pause, true),
            "3659cfe6" => ("upgradeTo(address)".to_string(), FuncType::Upgrade, true),
            _ => (format!("unknown_{}", hex_sel), FuncType::Unknown, false),
        }
    }
    
    /// 📋 Retorna perfil de token já analisado
    pub async fn get_token_profile(&self, address: &Address) -> Option<TokenRiskProfile> {
        self.analyzed_tokens.read().await.get(address).cloned()
    }
    
    /// ⚠️ Verifica se token é seguro para trading
    pub async fn is_safe_for_trading(&self, address: &Address) -> bool {
        if let Some(profile) = self.get_token_profile(address).await {
            profile.recommendation == RiskRecommendation::Safe
        } else {
            false // Sem análise = não seguro
        }
    }
    
    /// 📊 Estatísticas de análise
    pub async fn stats(&self) -> String {
        let analyses = *self.analyses_count.read().await;
        let threats = *self.threats_found.read().await;
        let tokens = self.analyzed_tokens.read().await.len();
        
        let safe_count = self.analyzed_tokens.read().await
            .values()
            .filter(|p| p.recommendation == RiskRecommendation::Safe)
            .count();
        
        let avoid_count = self.analyzed_tokens.read().await
            .values()
            .filter(|p| p.recommendation == RiskRecommendation::Avoid)
            .count();
        
        format!(
            "🔍 Bytecode Analyzer | Tokens: {} | Análises: {} | Ameaças: {} | Safe: {} | Avoid: {}",
            tokens, analyses, threats, safe_count, avoid_count
        )
    }
}

/// 🔎 Encontra padrão em bytecode
fn find_pattern(bytecode: &Bytes, pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() || bytecode.len() < pattern.len() {
        return None;
    }
    
    bytecode.windows(pattern.len())
        .position(|window| window == pattern)
}

/// 🧮 Calcula hash simples do bytecode
fn calculate_hash(bytecode: &Bytes) -> u64 {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    
    let mut hasher = DefaultHasher::new();
    bytecode.as_ref().hash(&mut hasher);
    hasher.finish()
}

use tracing::{info, warn};
