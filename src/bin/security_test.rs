//! Testes de segurança — anti-honeypot, anti-scam, proteção de capital
use orca_mev::security::honeypot_filter::{HoneypotFilter, TokenRisk};
use alloy::primitives::address;

fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║     🛡️  ORCA SECURITY TEST — Anti-Scam Suite     ║");
    println!("╚══════════════════════════════════════════════════╝");

    let mut passed = 0;
    let mut failed = 0;

    macro_rules! test {
        ($name:expr, $cond:expr) => {
            if $cond {
                println!("✅ {}", $name);
                passed += 1;
            } else {
                println!("❌ {}", $name);
                failed += 1;
            }
        };
    }

    let filter = HoneypotFilter::new();

    // T1: Tokens seguros conhecidos passam
    let weth = address!("4200000000000000000000000000000000000006");
    let usdc = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
    test!("T01_WETH_SAFE", filter.is_safe(weth));
    test!("T02_USDC_SAFE", filter.is_safe(usdc));

    // T2: Token marcado como honeypot é bloqueado
    let fake_token = address!("1111111111111111111111111111111111111111");
    filter.mark_dangerous(fake_token, TokenRisk::Honeypot);
    test!("T03_HONEYPOT_BLOCKED", !filter.is_safe(fake_token));

    // T3: Path com honeypot é bloqueado
    test!("T04_PATH_WITH_HONEYPOT_BLOCKED", !filter.is_path_safe(&[weth, fake_token, usdc]));

    // T4: Path limpo passa
    test!("T05_CLEAN_PATH_PASSES", filter.is_path_safe(&[weth, usdc]));

    // T5: Token com tax alta é bloqueado
    let tax_token = address!("2222222222222222222222222222222222222222");
    filter.mark_dangerous(tax_token, TokenRisk::TransferTax(1000)); // 10% tax
    test!("T06_HIGH_TAX_BLOCKED", !filter.is_safe(tax_token));

    // T6: Token com tax baixa passa
    let low_tax = address!("3333333333333333333333333333333333333333");
    filter.mark_dangerous(low_tax, TokenRisk::TransferTax(100)); // 1% tax
    test!("T07_LOW_TAX_PASSES", filter.is_safe(low_tax));

    // T7: Falha de execução marca token como suspeito
    let suspicious = address!("4444444444444444444444444444444444444444");
    filter.record_failed_execution(suspicious);
    test!("T08_FAILED_EXEC_MARKS_SUSPICIOUS", !filter.is_safe(suspicious));

    // T8: Sell blocked é bloqueado
    let sell_blocked = address!("5555555555555555555555555555555555555555");
    filter.mark_dangerous(sell_blocked, TokenRisk::SellBlocked);
    test!("T09_SELL_BLOCKED_REJECTED", !filter.is_safe(sell_blocked));

    // T9: Stats funcionam
    let stats = filter.stats();
    test!("T10_STATS_FUNCTIONAL", !stats.is_empty());

    println!("╔══════════════════════════════════════════════════╗");
    println!("║  ✅ Passed: {:2} | ❌ Failed: {:2}                 ║", passed, failed);
    if failed == 0 {
        println!("║  🛡️  CAPITAL PROTEGIDO — Filtros ativos          ║");
    } else {
        println!("║  ⚠️  ATENÇÃO — Verificar filtros antes de produção║");
    }
    println!("╚══════════════════════════════════════════════════╝");
}
