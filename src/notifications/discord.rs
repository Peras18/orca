use reqwest::Client;
use serde_json::json;
use tracing::warn;
use std::sync::Arc;
use crate::pricing::EthPriceFeed;

#[derive(Clone, Debug)]
pub struct DiscordNotifier {
    webhook_url: String,
    client: Client,
    price_feed: Arc<EthPriceFeed>,
}
impl DiscordNotifier {
    /// CORREÇÃO: deixou de criar o seu próprio cache de preço ETH/EUR
    /// (havia duas instâncias de DiscordNotifier no projecto -- orca/mod.rs
    /// e main.rs -- cada uma com cache independente, por vezes divergente).
    /// Agora recebe a fonte de preço partilhada via Arc, sincronizada com
    /// todo o resto da aplicação (JIT, performance_tracker, safety).
    pub fn new(webhook_url: &str, price_feed: Arc<EthPriceFeed>) -> Self {
        Self { webhook_url: webhook_url.to_string(), client: Client::new(), price_feed }
    }
    pub async fn get_eth_price_eur(&self) -> f64 {
        self.price_feed.get_eur().await
    }
    async fn send(&self, content: &str) {
        let payload = json!({"content": content});
        if let Err(e) = self.client.post(&self.webhook_url).json(&payload).send().await {
            warn!("[DISCORD] {}", e);
        }
    }
    pub async fn notify_start(&self) {
        let mode = if std::env::var("DRY_RUN").unwrap_or_default() == "false" { "LIVE 🔴" } else { "DRY-RUN 🟡" };
        self.send(&format!("🐋 **ORCA Engine iniciado** | Modo: {}", mode)).await;
    }
    pub async fn notify_stop(&self, opps: u64, total_eur: f64, melhor_eur: f64) {
        self.send(&format!("🛑 **Sessão terminada**\n• Opps: {}\n• Total: {:.2}€\n• Melhor: {:.2}€", opps, total_eur, melhor_eur)).await;
    }
    pub async fn notify_opportunity(&self, _path: &str, _profit_eur: f64, _hops: usize, _block: u64) {
        // Silenciado — só notificar lucros reais confirmados via notify_execution
    }
    pub async fn notify_execution(&self, tx_hash: &str, profit_eth: f64, loan_eth: f64, _unused: f64) {
        let eth_price_eur = self.get_eth_price_eur().await;
        let profit_eur = profit_eth * eth_price_eur;
        let loan_eur = loan_eth * eth_price_eur;
        self.send(&format!(
            "💸 **LUCRO CONFIRMADO**\n• Lucro: {:.6} ETH ({:.2}€)\n• Flash Loan: {:.4} ETH ({:.2}€)\n• TX: `{}`",
            profit_eth, profit_eur, loan_eth, loan_eur, tx_hash
        )).await;
    }
    pub async fn notify_daily_summary(&self, opps: u64, total_eur: f64, melhor_eur: f64, media_eur: f64) {
        self.send(&format!("📈 **Resumo do dia**\n• Execuções: {}\n• Total: {:.2}€\n• Média: {:.2}€\n• Melhor: {:.2}€", opps, total_eur, media_eur, melhor_eur)).await;
    }
    pub async fn notify_error(&self, error: &str) {
        self.send(&format!("🚨 **ERRO**\n```{}```", error)).await;
    }
    pub async fn notify_heartbeat(&self, block: u64, opps_hora: u64, profit_hora: f64) {
        self.send(&format!("🩻 Bloco {} | {}/h execuções | {:.2}€/h", block, opps_hora, profit_hora)).await;
    }
}
