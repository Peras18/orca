//! 💶 EthPriceFeed — fonte única de verdade para a taxa ETH/EUR
//!
//! Antes desta struct existirem 7 valores hardcoded e divergentes
//! (1600.0, 1800.0, 3000.0, 3500.0...) espalhados por orca/mod.rs,
//! performance_tracker.rs e notifications/discord.rs -- cada módulo
//! decidia o lucro/TVL/threshold com um preço de ETH diferente ao
//! mesmo tempo. Esta struct substitui todos esses pontos por uma
//! única chamada à CoinGecko, cacheada 60s, partilhada via Arc.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::warn;

#[derive(Debug)]
pub struct EthPriceFeed {
    client: reqwest::Client,
    cache: RwLock<(f64, Instant)>,
}

impl EthPriceFeed {
    /// Seed inicial conservador, só usado até à primeira chamada real
    /// à API ter sucesso (ou se a API falhar repetidamente no arranque).
    const SEED_PRICE_EUR: f64 = 1800.0;
    const CACHE_TTL: Duration = Duration::from_secs(60);

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::new(),
            cache: RwLock::new((
                Self::SEED_PRICE_EUR,
                Instant::now() - Duration::from_secs(120), // já expirado -- força fetch real na 1ª chamada
            )),
        })
    }

    /// Devolve o preço ETH/EUR mais recente. Usa cache de 60s; se a
    /// chamada à API falhar, devolve o último preço válido conhecido
    /// (nunca um erro) para não bloquear decisões de threshold.
    pub async fn get_eur(&self) -> f64 {
        {
            let cache = self.cache.read().await;
            if cache.1.elapsed() < Self::CACHE_TTL {
                return cache.0;
            }
        }

        let url = "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=eur";
        match self
            .client
            .get(url)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    if let Some(price) = json
                        .get("ethereum")
                        .and_then(|e| e.get("eur"))
                        .and_then(|p| p.as_f64())
                    {
                        let mut cache = self.cache.write().await;
                        *cache = (price, Instant::now());
                        return price;
                    }
                    warn!("[EthPriceFeed] resposta CoinGecko sem campo eur -- a usar último preço cacheado");
                }
                Err(e) => warn!("[EthPriceFeed] falha a parsear resposta CoinGecko: {} -- a usar último preço cacheado", e),
            },
            Err(e) => warn!("[EthPriceFeed] falha a contactar CoinGecko: {} -- a usar último preço cacheado", e),
        }

        // Falha na API: devolve o último preço cacheado (mesmo expirado)
        // em vez de propagar erro -- preferível decidir com preço levemente
        // desactualizado do que travar a pipeline de detecção/execução.
        self.cache.read().await.0
    }
}
