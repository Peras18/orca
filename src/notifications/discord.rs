use reqwest::Client;
use serde_json::json;
use tracing::warn;

#[derive(Clone, Debug)]
pub struct DiscordNotifier {
    webhook_url: String,
    client: Client,
}

impl DiscordNotifier {
    pub fn new(webhook_url: &str) -> Self {
        Self { webhook_url: webhook_url.to_string(), client: Client::new() }
    }

    async fn send(&self, content: &str) {
        let payload = json!({"content": content});
        if let Err(e) = self.client.post(&self.webhook_url).json(&payload).send().await {
            warn!("[DISCORD] {}", e);
        }
    }

    pub async fn notify_start(&self) {
        self.send("🐋 **ORCA Engine iniciado** | Sessão: 07h45 → 21h30 PT | Modo: DRY-RUN").await;
    }

    pub async fn notify_stop(&self, opps: u64, total_eur: f64, melhor_eur: f64) {
        self.send(&format!("🛑 **Sessão terminada**\n• Opps: {}\n• Total: {:.2}€\n• Melhor: {:.2}€", opps, total_eur, melhor_eur)).await;
    }

    pub async fn notify_opportunity(&self, path: &str, profit_eur: f64, hops: usize, block: u64) {
        if profit_eur < 1.0 { return; }
        self.send(&format!("🎯 **Opp {:.2}€** | {} hops | bloco {}\n`{}`", profit_eur, hops, block, path)).await;
    }

    pub async fn notify_daily_summary(&self, opps: u64, total_eur: f64, melhor_eur: f64, media_eur: f64) {
        self.send(&format!("📈 **Resumo do dia**\n• Opps: {}\n• Total: {:.2}€\n• Média: {:.2}€\n• Melhor: {:.2}€", opps, total_eur, media_eur, melhor_eur)).await;
    }

    pub async fn notify_error(&self, error: &str) {
        self.send(&format!("🚨 **ERRO**\n```{}```", error)).await;
    }

    pub async fn notify_heartbeat(&self, block: u64, opps_hora: u64, profit_hora: f64) {
        self.send(&format!("💓 Bloco {} | {}/h opps | {:.2}€/h", block, opps_hora, profit_hora)).await;
    }
}
