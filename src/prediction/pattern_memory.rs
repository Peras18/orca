use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alloy::primitives::Address;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Clone, Debug)]
pub struct PatternMemory {
    records: Arc<DashMap<String, Vec<(u8, u128)>>>,
    storage_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct PatternSnapshot {
    records: HashMap<String, Vec<(u8, u128)>>,
}

impl PatternMemory {
    pub fn new(storage_path: impl Into<PathBuf>) -> Self {
        let memory = Self {
            records: Arc::new(DashMap::new()),
            storage_path: storage_path.into(),
        };
        memory.load_from_disk();
        memory
    }

    pub fn record_opportunity(&self, pool: Address, hour_of_day: u8, profit_wei: u128) {
        if hour_of_day > 23 {
            return;
        }

        let key = format!("{pool:?}");
        let mut entry = self.records.entry(key).or_default();
        entry.push((hour_of_day, profit_wei));
        if entry.len() > 512 {
            let drain_len = entry.len().saturating_sub(512);
            entry.drain(0..drain_len);
        }
    }

    pub fn pool_score_now(&self, pool: Address, hour_of_day: u8) -> f64 {
        let key = format!("{pool:?}");
        let Some(entry) = self.records.get(&key) else {
            return 0.0;
        };

        let mut total_profit: u128 = 0;
        let mut matches: u32 = 0;
        for (h, profit) in entry.iter() {
            if *h == hour_of_day {
                total_profit = total_profit.saturating_add(*profit);
                matches += 1;
            }
        }

        if matches == 0 {
            return 0.0;
        }
        (total_profit as f64 / matches as f64) / 1e18
    }

    pub fn to_priority_map(&self, pools: &[Address], hour_of_day: u8) -> HashMap<Address, f64> {
        let mut out = HashMap::with_capacity(pools.len());
        for pool in pools {
            out.insert(*pool, self.pool_score_now(*pool, hour_of_day));
        }
        out
    }

    pub fn persist_to_disk(&self) {
        let mut snapshot = HashMap::with_capacity(self.records.len());
        for item in self.records.iter() {
            snapshot.insert(item.key().clone(), item.value().clone());
        }

        let payload = PatternSnapshot { records: snapshot };
        let Ok(json) = serde_json::to_string_pretty(&payload) else {
            warn!("[PATTERN] Falha ao serializar snapshot");
            return;
        };

        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Err(err) = fs::write(&self.storage_path, json) {
            warn!(
                "[PATTERN] Falha ao persistir memória em {}: {}",
                self.storage_path.display(),
                err
            );
        }
    }

    fn load_from_disk(&self) {
        let path: &Path = &self.storage_path;
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(snapshot) = serde_json::from_str::<PatternSnapshot>(&raw) else {
            warn!("[PATTERN] Snapshot inválido em {}", path.display());
            return;
        };

        for (pool, history) in snapshot.records {
            self.records.insert(pool, history);
        }
    }
}
