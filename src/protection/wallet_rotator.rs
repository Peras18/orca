//! 🔄 WalletRotator — Sistema de anonimato de transações
//!
//! Rota transações por múltiplas carteiras para evitar fingerprinting
//! por builders e searchers concorrentes. Cada carteira é usada
//! de forma pseudo-aleatória com pesos de confiança.

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Par (endereço, chave privada) para submissão de bundles
#[derive(Clone)]
pub struct RotatorWallet {
    pub address: Address,
    pub signer: Arc<PrivateKeySigner>,
    pub use_count: u64,
    pub last_used: std::time::Instant,
}

/// Rota carteiras para dificultar fingerprinting
pub struct WalletRotator {
    wallets: RwLock<Vec<RotatorWallet>>,
    current_index: RwLock<usize>,
    max_uses_per_wallet: u64,
    cooldown_secs: u64,
}

impl WalletRotator {
    pub fn new(wallets: Vec<(Address, Arc<PrivateKeySigner>)>, max_uses: u64, cooldown: u64) -> Self {
        let wallets = wallets
            .into_iter()
            .map(|(addr, signer)| RotatorWallet {
                address: addr,
                signer,
                use_count: 0,
                last_used: std::time::Instant::now() - std::time::Duration::from_secs(cooldown + 1),
            })
            .collect();

        Self {
            wallets: RwLock::new(wallets),
            current_index: RwLock::new(0),
            max_uses_per_wallet: max_uses,
            cooldown_secs: cooldown,
        }
    }

    /// Seleciona a próxima carteira usando estratégia ponderada
    /// que favorece carteiras menos usadas e com mais tempo de cooldown
    pub async fn next_wallet(&self) -> Option<RotatorWallet> {
        let mut wallets = self.wallets.write().await;
        let now = std::time::Instant::now();

        // Filtrar carteiras disponíveis (cooldown respeitado)
        let mut candidates: Vec<usize> = wallets
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                let elapsed = now.duration_since(w.last_used).as_secs();
                elapsed >= self.cooldown_secs && w.use_count < self.max_uses_per_wallet
            })
            .map(|(i, _)| i)
            .collect();

        if candidates.is_empty() {
            // Se nenhuma carteira disponível, resetar cooldown e usar a menos usada
            candidates = (0..wallets.len()).collect();
        }

        // Shuffle aleatório para não ser previsível
        candidates.shuffle(&mut thread_rng());

        // Escolher a carteira menos usada entre as candidatas
        let best = candidates
            .iter()
            .min_by_key(|&&i| wallets[i].use_count)
            .copied()?;

        wallets[best].use_count += 1;
        wallets[best].last_used = now;

        debug!("🔄 WalletRotator: usando {:?} (usos: {})", wallets[best].address, wallets[best].use_count);

        Some(wallets[best].clone())
    }

    /// Retorna o número total de submissões feitas
    pub async fn total_submissions(&self) -> u64 {
        let wallets = self.wallets.read().await;
        wallets.iter().map(|w| w.use_count).sum()
    }

    /// Regenera pesos aleatórios para confundir análise de padrão
    pub async fn shuffle_weights(&self) {
        let mut wallets = self.wallets.write().await;
        // Adicionar jitter aos use_count para quebrar padrões estatísticos
        for w in wallets.iter_mut() {
            if w.use_count > 0 {
                w.use_count = w.use_count.saturating_sub(1);
            }
        }
        info!("🔄 WalletRotator: pesos regenerados");
    }
}

/// Wallet singleton para uso quando rotator não está configurado
pub async fn get_submission_wallet(
    rotator: Option<&WalletRotator>,
    fallback: Arc<PrivateKeySigner>,
) -> (Address, Arc<PrivateKeySigner>) {
    if let Some(r) = rotator {
        if let Some(w) = r.next_wallet().await {
            return (w.address, w.signer);
        }
    }
    let addr = fallback.address();
    (addr, fallback)
}
