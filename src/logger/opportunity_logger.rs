use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::collections::HashSet;
use chrono::Utc;

#[derive(Debug)]
pub struct OpportunityLogger {
    file: Mutex<std::fs::File>,
    seen: Mutex<(u64, HashSet<String>)>,
}

pub struct OpportunityRecord {
    pub block: u64,
    pub path: String,
    pub hops: usize,
    pub input_wei: u128,
    pub gross_profit_wei: u128,
    pub net_profit_wei: u128,
    pub gas_cost_wei: u128,
}

impl OpportunityLogger {
    pub fn new(path: &str) -> Self {
        let exists = Path::new(path).exists();
        let file = OpenOptions::new().create(true).append(true).open(path)
            .expect("CSV open failed");
        if !exists {
            let mut f = file.try_clone().expect("clone failed");
            writeln!(f, "timestamp,block,path,hops,input_eth,gross_profit_eth,gas_cost_eth,net_profit_eth,net_profit_eur_1800").ok();
        }
        Self {
            file: Mutex::new(file),
            seen: Mutex::new((0, HashSet::new())),
        }
    }

    pub fn log(&self, r: &OpportunityRecord) {
        {
            let mut seen = self.seen.lock().unwrap();
            if seen.0 != r.block {
                seen.0 = r.block;
                seen.1.clear();
            }
            if seen.1.contains(&r.path) { return; }
            seen.1.insert(r.path.clone());
        }
        let ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut f = self.file.lock().unwrap();
        writeln!(f, "{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.4}",
            ts, r.block, r.path, r.hops,
            r.input_wei as f64 / 1e18,
            r.gross_profit_wei as f64 / 1e18,
            r.gas_cost_wei as f64 / 1e18,
            r.net_profit_wei as f64 / 1e18,
            r.net_profit_wei as f64 / 1e18 * 1800.0,
        ).ok();
    }
}
