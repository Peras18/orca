//! ORCA Backtester — src/bin/backtester.rs
#![allow(dead_code)]

use alloy::primitives::{address, Address, U256};
use chrono::{NaiveTime, Utc};
use eyre::{eyre, Result, WrapErr};
use reqwest::Client;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook};
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{info, warn};

use orca_mev::cache::pool_cache::{PoolCache, PoolState};
use orca_mev::contracts::{DexType, SYNC_TOPIC0};
use orca_mev::graph::arb_graph::ArbGraph;

const WETH: Address = address!("4200000000000000000000000000000000000006");
const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
const FLASH_AMOUNTS_ETH: &[f64] = &[0.1, 0.5, 1.0, 5.0, 10.0];
const LOG_CHUNK_BLOCKS: u64 = 10;
fn seed_pools() -> Vec<(Address, Address, Address, u32, DexType)> {
    vec![
        (address!("88A43bbDF9D098eEC7bCEda4e2494615dfD9bB9C"), WETH, USDC, 25, DexType::UniswapV2),
        (address!("cDAC0d6c6C59727a65F871236188350531885C43"), WETH, USDC, 30, DexType::Aerodrome),
        (address!("a1d2ee6212d4375f154393e59895a5d27859c4e4"), USDC, address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"), 30, DexType::Aerodrome),
        (address!("2578365b3dfa7ffe60108e181efb79feddec2319"), WETH, address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"), 30, DexType::Aerodrome),
        (address!("96508ae8037c6bd16162620187691f1c1e3e07c1"), USDC, address!("fde4C96c8593536E31F229EA8f37b2ADa2699bb2"), 30, DexType::Aerodrome),
        (address!("6cDcb1C4A4D1C3C6d054b27AC5B77e89eAFb971d"), WETH, USDC, 30, DexType::Aerodrome),
        (address!("67b00B46FA4f4F24c03855c5C8013C0B938B3eEc"), address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"), WETH, 30, DexType::Aerodrome),
        (address!("6b2a379f803923542047ac2c7f268ffe7989d869"), USDC, address!("fde4C96c8593536E31F229EA8f37b2ADa2699bb2"), 30, DexType::UniswapV2),
        (address!("89d0f320ac73dd7d9513ffc5bc58d1161452a657"), WETH, USDC, 30, DexType::Aerodrome),
        (address!("b0d931138bf96501654f2268cdd84420151ff52e"), WETH, USDC, 30, DexType::Aerodrome),
        (address!("01784ef301d79e4b2df3a21ad9a536d4cf09a5ce"), WETH, USDC, 30, DexType::Aerodrome),
        (address!("ae37030302643ee121dfa50e76b27b90fac9e872"), WETH, USDC, 30, DexType::Aerodrome),
    ]
}

struct Rpc {
    client: Client,
    url: String,
    id: std::sync::atomic::AtomicU64,
}

impl Rpc {
    fn new(url: String) -> Self {
        Self {
            client: Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap(),
            url,
            id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let resp = self.client.post(&self.url).json(&body).send().await.wrap_err("RPC send")?;
        let json: Value = resp.json().await.wrap_err("RPC parse")?;
        if let Some(e) = json.get("error") {
            let code = e.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            if code == 429 || code == -32005 { return Ok(json!([]));
            }
            return Err(eyre!("RPC error: {}", e));
        }
        json.get("result").cloned().ok_or_else(|| eyre!("No result"))
    }

    async fn block_number(&self) -> Result<u64> {
        hex_to_u64(self.call("eth_blockNumber", json!([])).await?.as_str().unwrap_or("0x0"))
    }

    async fn block_timestamp(&self, block: u64) -> Result<u64> {
        let v = self.call("eth_getBlockByNumber", json!([format!("0x{:x}", block), false])).await?;
        hex_to_u64(v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("0x0"))
    }

    async fn base_fee_at(&self, block: u64) -> Result<u64> {
        let v = self.call("eth_getBlockByNumber", json!([format!("0x{:x}", block), false])).await?;
        hex_to_u64(v.get("baseFeePerGas").and_then(|t| t.as_str()).unwrap_or("0x1DCD6500"))
    }

    async fn block_near_timestamp(&self, target_ts: u64) -> Result<u64> {
        let latest = self.block_number().await?;
        let latest_ts = self.block_timestamp(latest).await?;
        if target_ts >= latest_ts { return Ok(latest);
        }
        let delta = latest_ts.saturating_sub(target_ts);
        let est = latest.saturating_sub(delta / 2);
        let mut lo = est.saturating_sub(500);
        let mut hi = (est + 500).min(latest);
        for _ in 0..6 {
            let mid = (lo + hi) / 2;
            let mid_ts = self.block_timestamp(mid).await?;
            if mid_ts < target_ts { lo = mid + 1; } else { hi = mid;
            }
        }
        Ok(lo)
    }

    async fn get_sync_logs(&self, from: u64, to: u64) -> Result<Vec<Value>> {
        let topic = format!("0x{}", hex::encode(SYNC_TOPIC0.as_slice()));
        let v = self.call("eth_getLogs", json!([{
            "fromBlock": format!("0x{:x}", from),
            "toBlock":   format!("0x{:x}", to),
            "topics":    [topic]
        }])).await?;
        if v.is_array() {
            Ok(v.as_array().cloned().unwrap_or_default())
        } else {
            Ok(vec![])
        }
    }
}

fn hex_to_u64(s: &str) -> Result<u64> {
    Ok(u64::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

fn wei_to_eth(wei: U256) -> f64 {
    (wei / U256::from(10u64.pow(15))).to::<u128>() as f64 / 1_000.0
}

fn eth_to_wei(eth: f64) -> U256 {
    U256::from((eth * 1e18) as u128)
}

fn trading_window_ts(day: &chrono::NaiveDate) -> (u64, u64) {
    let pt_offset_secs: i64 = 
    7 * 3600;
    let start = day.and_time(NaiveTime::from_hms_opt(7, 45, 0).unwrap())
        .and_utc().timestamp() + pt_offset_secs;
    let end = day.and_time(NaiveTime::from_hms_opt(22, 0, 0).unwrap())
        .and_utc().timestamp() + pt_offset_secs;
    (start as u64, end as u64)
}

fn apply_sync_log(
    log: &Value,
    cache: &PoolCache,
    meta: &HashMap<Address, (Address, Address, u32, DexType)>,
) -> Option<u64> {
    let pool_addr: Address = log.get("address")?.as_str()?.parse().ok()?;
    if !meta.contains_key(&pool_addr) { return None; }
    let data = log.get("data")?.as_str()?.trim_start_matches("0x");
    if data.len() < 128 { return None;
    }
    let r0 = U256::from_be_slice(&hex::decode(&data[0..64]).ok()?);
    let r1 = U256::from_be_slice(&hex::decode(&data[64..128]).ok()?);
    let block = hex_to_u64(log.get("blockNumber")?.as_str()?).ok()?;
    if !cache.contains(&pool_addr) {
        if let Some(&(t0, t1, fee, dex)) = meta.get(&pool_addr) {
            let mut s = PoolState::new(pool_addr, t0, t1, fee, dex);
            s.decimals0 = if t0 == USDC { 6 } else { 18 };
            s.decimals1 = if t1 == USDC { 6 } else { 18 };
            cache.insert(s);
        }
    }
    cache.update_sync_event(pool_addr, r0, r1, block);
    Some(block)
}

#[derive(Clone)]
struct Trade {
    block: u64,
    date: String,
    path: String,
    flash_eth: f64,
    gross_eth: f64,
    gas_eth: f64,
    net_eth: f64,
    gas_gwei: f64,
    hops: usize,
}

#[derive(Clone, Default)]
struct DaySummary {
    date: String,
    blocks_scanned: usize,
    opps: usize,
    profitable: usize,
    gross_eth: f64,
    gas_eth: f64,
    net_eth: f64,
    best_eth: f64,
}

fn export_excel(trades: &[Trade], days: &[DaySummary], path: &str) -> Result<()> {
    let mut wb = Workbook::new();
    let hdr = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0x1B2A4A))
        .set_font_color(Color::White)
        .set_align(FormatAlign::Center)
        .set_border(FormatBorder::Thin);
    let pos  = Format::new().set_num_format("0.000000").set_font_color(Color::RGB(0x005500));
    let neg  = Format::new().set_num_format("0.000000").set_font_color(Color::Red);
    let num  = Format::new().set_num_format("0.000000");
    let pct  = Format::new().set_num_format("0.00%");
    let bold = Format::new().set_bold();

    {
        let ws = wb.add_worksheet();
        ws.set_name("Trades")?;
        ws.set_freeze_panes(1, 0)?;
        let cols: &[(&str, f64)] = &[
            ("Bloco",12.),("Data",13.),("Path",60.),("Flash ETH",12.),
            ("Gross ETH",13.),("Gas ETH",12.),("Net ETH",13.),("Gas Gwei",12.),("Hops",7.),
        ];
        for (i,(h,w)) in cols.iter().enumerate() {
            ws.set_column_width(i as u16, *w)?;
            ws.write_with_format(0, i as u16, *h, &hdr)?;
        }
        for (i,t) in trades.iter().enumerate() {
            let row = (i+1) as u32;
            ws.write_number(row, 0, t.block as f64)?;
            ws.write_string(row, 1, &t.date)?;
            ws.write_string(row, 2, &t.path)?;
            ws.write_number_with_format(row, 3, t.flash_eth, &num)?;
            ws.write_number_with_format(row, 4, t.gross_eth, &num)?;
            ws.write_number_with_format(row, 5, t.gas_eth,   &num)?;
            ws.write_number_with_format(row, 6, t.net_eth, if t.net_eth >= 0. { &pos } else { &neg })?;
            ws.write_number_with_format(row, 7, t.gas_gwei,  &num)?;
            ws.write_number(row, 8, t.hops as f64)?;
        }
    }

    {
        let ws = wb.add_worksheet();
        ws.set_name("Resumo Diário")?;
        ws.set_freeze_panes(1, 0)?;
        let cols: &[(&str, f64)] = &[
            ("Data",13.),("Blocos c/Eventos",17.),("Oportunidades",16.),
            ("Lucrativas",13.),("Taxa",9.),
            ("Gross ETH",13.),("Gas ETH",12.),("Net ETH",13.),("Melhor ETH",13.),
        ];
        for (i,(h,w)) in cols.iter().enumerate() {
            ws.set_column_width(i as u16, *w)?;
            ws.write_with_format(0, i as u16, *h, &hdr)?;
        }
        for (i,d) in days.iter().enumerate() {
            let row = (i+1) as u32;
            let rate = if d.opps > 0 { d.profitable as f64 / d.opps as f64 } else { 0. };
            ws.write_string(row, 0, &d.date)?;
            ws.write_number(row, 1, d.blocks_scanned as f64)?;
            ws.write_number(row, 2, d.opps as f64)?;
            ws.write_number(row, 3, d.profitable as f64)?;
            ws.write_number_with_format(row, 4, rate, &pct)?;
            ws.write_number_with_format(row, 5, d.gross_eth, &num)?;
            ws.write_number_with_format(row, 6, d.gas_eth,   &num)?;
            ws.write_number_with_format(row, 7, d.net_eth, if d.net_eth >= 0. { &pos } else { &neg })?;
            ws.write_number_with_format(row, 8, d.best_eth,  &pos)?;
        }
        let tr = (days.len() + 2) as u32;
        ws.write_with_format(tr, 0, "TOTAL 7d", &bold)?;
        ws.write_number(tr, 2, days.iter().map(|d| d.opps).sum::<usize>() as f64)?;
        ws.write_number(tr, 3, days.iter().map(|d| d.profitable).sum::<usize>() as f64)?;
        ws.write_number_with_format(tr, 5, days.iter().map(|d| d.gross_eth).sum::<f64>(), &bold)?;
        ws.write_number_with_format(tr, 6, days.iter().map(|d| d.gas_eth).sum::<f64>(), &bold)?;
        ws.write_number_with_format(tr, 7, days.iter().map(|d| d.net_eth).sum::<f64>(), &bold)?;
    }

    {
        let ws = wb.add_worksheet();
        ws.set_name("Config")?;
        ws.set_column_width(0, 26.)?;
        ws.set_column_width(1, 52.)?;
        let rows: &[(&str, &str)] = &[
            ("Chain",          "Base Mainnet (8453)"),
            ("Janela Diária",  "07:45-22:00 PT  =  14:45-05:00 UTC"),
            ("Flash Amounts",  "0.1 / 0.5 / 1 / 5 / 10 ETH"),
            ("Matematica AMM", "get_amount_out_v2/v3 — identica a arb_graph.rs"),
            ("DEXs",           "UniswapV2 / Aerodrome vAMM"),
            ("Dados",          "eth_getLogs Sync events — Alchemy (sem filtro addr)"),
        ];
        for (i,(k,v)) in rows.iter().enumerate() {
            ws.write_with_format(i as u32, 0, *k, &hdr)?;
            ws.write_string(i as u32, 1, *v)?;
        }
        ws.write_with_format(rows.len() as u32, 0, "Gerado", &hdr)?;
        ws.write_string(rows.len() as u32, 1, &Utc::now().format("%Y-%m-%d %H:%M UTC").to_string())?;
    }

    wb.save(path)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("backtester=info".parse().unwrap())
                .add_directive("orca_mev=warn".parse().unwrap()),
        )
        .with_target(false)
        .init();
    let rpc_url = std::env::var("RPC_HTTP_URLS")
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string())
        .or_else(|_| std::env::var("BACKTEST_RPC_URL"))
        .map_err(|_| eyre!("Define RPC_HTTP_URLS no .env"))?;
    if rpc_url.is_empty() { return Err(eyre!("RPC_HTTP_URLS vazio")); }

    let days_count: i64 = std::env::var("BACKTEST_DAYS")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(7);
    let output = std::env::var("BACKTEST_OUT")
        .unwrap_or_else(|_| "orca_backtest.xlsx".to_string());
    info!("ORCA Backtester — {} dias | {}", days_count, output);

    let rpc = Rpc::new(rpc_url);
    let seeds = seed_pools();
    let mut pool_meta: HashMap<Address, (Address, Address, u32, DexType)> = HashMap::new();
    for &(pool, t0, t1, fee, dex) in &seeds {
        pool_meta.insert(pool, (t0, t1, fee, dex));
    }

    let flash_amounts: Vec<U256> = FLASH_AMOUNTS_ETH.iter().map(|&e| eth_to_wei(e)).collect();
    let mut all_trades: Vec<Trade> = Vec::new();
    let mut all_days:   Vec<DaySummary> = Vec::new();
    let today = Utc::now().date_naive();
    for day_idx in 0..days_count {
        let day = today - chrono::Days::new((days_count - day_idx) as u64);
        let (win_start, win_end) = trading_window_ts(&day);

        info!("Dia {}/{} — {} [07:45-22:00 PT]", day_idx+1, days_count, day);
        let from_block = match rpc.block_near_timestamp(win_start).await {
            Ok(b) => b, Err(e) => { warn!("Bloco inicial: {}", e);
            continue; }
        };
        let to_block = match rpc.block_near_timestamp(win_end).await {
            Ok(b) => b, Err(e) => { warn!("Bloco final: {}", e);
            continue; }
        };
        if to_block <= from_block { warn!("Range invalido"); continue;
        }
        info!("  Blocos {} -> {} ({} blocos)", from_block, to_block, to_block - from_block);
        let cache = PoolCache::new();
        for &(pool, t0, t1, fee, dex) in &seeds {
            let mut s = PoolState::new(pool, t0, t1, fee, dex);
            s.decimals0 = if t0 == USDC { 6 } else { 18 };
            s.decimals1 = if t1 == USDC { 6 } else { 18 };
            cache.insert(s);
        }

        let mut logs_by_block: HashMap<u64, Vec<Value>> = HashMap::new();
        let mut cursor = from_block;
        let mut chunks_ok = 0usize;
        let mut chunks_skipped = 0usize;
        while cursor <= to_block {
            let chunk_end = (cursor + LOG_CHUNK_BLOCKS - 1).min(to_block);
            match rpc.get_sync_logs(cursor, chunk_end).await {
                Ok(logs) => {
                    if !logs.is_empty() { chunks_ok += 1;
                    }
                    for log in logs {
                        if let Some(bn_str) = log.get("blockNumber").and_then(|v| v.as_str()) {
                            if let Ok(bn) = hex_to_u64(bn_str) {
                                 logs_by_block.entry(bn).or_default().push(log);
                            }
                        }
                    }
                }
                Err(_) => { chunks_skipped += 1;
                }
            }
            cursor = chunk_end + 1;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }

        info!("  {} blocos com eventos Sync (chunks ok={} skip={})",
            logs_by_block.len(), chunks_ok, chunks_skipped);
        let mut day_sum = DaySummary { date: day.to_string(), ..Default::default() };
        let mut sorted_blocks: Vec<u64> = logs_by_block.keys().cloned().collect();
        sorted_blocks.sort_unstable();
        for &block_num in &sorted_blocks {
            for log in &logs_by_block[&block_num] {
                apply_sync_log(log, &cache, &pool_meta);
            }
            day_sum.blocks_scanned += 1;

            let gas_wei = rpc.base_fee_at(block_num).await.unwrap_or(500_000_000);
            let gas_gwei = gas_wei as f64 / 1e9;

            let mut graph = ArbGraph::new(cache.clone(), U256::ZERO);
            graph.rebuild(block_num);
            let opps = graph.find_opportunities(WETH, &flash_amounts, U256::from(gas_wei), 1.05);
            day_sum.opps += opps.len();
            for opp in &opps {
                let gross = wei_to_eth(opp.gross_profit);
                let gas   = wei_to_eth(opp.gas_cost);
                let net   = wei_to_eth(opp.net_profit);
                let flash = wei_to_eth(opp.input_amount);
                let path_desc = opp.hops.iter()
                    .map(|h| format!("{}", &format!("{:?}", h.pool)[2..8]))
                    .collect::<Vec<_>>().join("->");
                if net > 0.0 {
                    day_sum.profitable += 1;
                    day_sum.net_eth += net;
                    if net > day_sum.best_eth { day_sum.best_eth = net;
                    }
                }
                day_sum.gross_eth += gross;
                day_sum.gas_eth   += gas;

                all_trades.push(Trade {
                    block: block_num, date: day.to_string(), path: path_desc,
                    flash_eth: flash, gross_eth: gross, gas_eth: gas,
                    net_eth: net, gas_gwei, hops: opp.hops.len(),
                });
            }
        }

        info!("  {} oportunidades | {} lucrativas | Net: {:.6} ETH",
            day_sum.opps, day_sum.profitable, day_sum.net_eth);
        all_days.push(day_sum);
    }

    all_trades.sort_by(|a,b| b.net_eth.partial_cmp(&a.net_eth).unwrap_or(std::cmp::Ordering::Equal));

    let total_net: f64  = all_days.iter().map(|d| d.net_eth).sum();
    let total_prof: usize = all_days.iter().map(|d| d.profitable).sum();
    info!("Exportando {} registos para {}", all_trades.len(), output);
    export_excel(&all_trades, &all_days, &output)?;
    info!("Completo | {} lucrativos | Net 7d: {:.6} ETH | {}", total_prof, total_net, output);
    Ok(())
}