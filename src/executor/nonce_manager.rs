//! 🔢 Nonce Manager — Atomic nonce tracking with replace-by-fee
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::Mutex;
use tracing::{debug, info};
use alloy::primitives::B256;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct PendingTx {
    pub nonce: u64,
    pub tx_hash: B256,
    pub priority_fee: u128,
    pub submitted_at: std::time::Instant,
    pub max_wait_secs: u64,
}

pub struct NonceManager {
    next_nonce: AtomicU64,
    pending: Mutex<HashMap<u64, PendingTx>>,
    confirmed_nonce: AtomicU64,
    replace_bump_bps: u32,
}

impl NonceManager {
    pub fn new(starting: u64) -> Self {
        Self {
            next_nonce: AtomicU64::new(starting),
            pending: Mutex::new(HashMap::new()),
            confirmed_nonce: AtomicU64::new(starting.saturating_sub(1)),
            replace_bump_bps: 110,
        }
    }

    pub fn reserve(&self) -> u64 {
        self.next_nonce.fetch_add(1, Ordering::SeqCst)
    }

    pub fn register(&self, nonce: u64, tx_hash: B256, priority_fee: u128, max_wait: u64) {
        let tx = PendingTx {
            nonce,
            tx_hash,
            priority_fee,
            submitted_at: std::time::Instant::now(),
            max_wait_secs: max_wait,
        };
        self.pending.lock().insert(nonce, tx);
        info!("🔢 Nonce {} registered | Hash: {:?}", nonce, tx_hash);
    }

    pub fn check_replace(&self, nonce: u64) -> Option<u128> {
        let pending = self.pending.lock();
        let tx = pending.get(&nonce)?;
        if tx.submitted_at.elapsed().as_secs() < tx.max_wait_secs / 2 {
            return None;
        }
        let new_fee = (tx.priority_fee * self.replace_bump_bps as u128) / 100;
        Some(new_fee)
    }

    pub fn confirm(&self, nonce: u64) {
        self.confirmed_nonce.fetch_max(nonce, Ordering::SeqCst);
        self.pending.lock().remove(&nonce);
        debug!("🔢 Nonce {} confirmed", nonce);
    }
}
