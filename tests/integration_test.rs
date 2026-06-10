//! Testes de Integração para APEX-SHADOW-PROTOCOL
//! 
//! Estes testes verificam o comportamento real do sistema

#[cfg(test)]
mod tests {
    use std::time::Instant;
    use alloy::primitives::{Address, U256};
    
    /// Teste de latência do DNA Scanner
    #[tokio::test]
    async fn test_dna_scanner_latency() {
        use apex_base_mev::apex_shadow_protocol::{DnaScanner, ThreatLevel};
        
        let scanner = DnaScanner::new();
        let token = Address::new([0x42; 20]);
        let bytecode = vec![0x60; 500]; // 500 bytes
        
        let start = Instant::now();
        let report = scanner.scan(token, &bytecode).await;
        let elapsed = start.elapsed();
        
        println!("[LATENCY] DNA Scan: {:?}", elapsed);
        println!("[RESULT] Safe: {:?} | Level: {:?}", report.is_safe, report.threat_level);
        
        // Verificar se está dentro do limite de 10ms
        assert!(elapsed.as_millis() < 10, "DNA Scanner deve ser < 10ms");
    }
    
    /// Teste de Newton-Raphson
    #[test]
    fn test_newton_raphson_speed() {
        use apex_base_mev::god_mode::NewtonRaphsonOptimizer;
        
        let reserve_in = U256::from(100_000_000_000_000_000_000_000u128);
        let reserve_out = U256::from(150_000_000_000_000_000_000_000u128);
        
        let start = Instant::now();
        let result = NewtonRaphsonOptimizer::calculate_optimal_input(
            reserve_in,
            reserve_out,
            30, // fee
            U256::from(200_000_000_000_000u128), // gas
            0, // flash fee
        );
        let elapsed = start.elapsed();
        
        println!("[LATENCY] Newton-Raphson: {:?}", elapsed);
        if let Some((input, profit, iters)) = result {
            println!("[RESULT] Input: {} | Profit: {} | Iters: {}", input, profit, iters);
        }
        
        assert!(elapsed.as_micros() < 1000, "Newton-Raphson deve ser < 1ms");
    }
    
    /// Teste de concorrência com DashMap
    #[tokio::test]
    async fn test_concurrent_access() {
        use std::sync::Arc;
        use tokio::task;
        use apex_base_mev::apex_shadow_protocol::DnaScanner;
        
        let scanner = Arc::new(DnaScanner::new());
        let mut handles = vec![];
        
        let start = Instant::now();
        
        // 100 acessos concorrentes
        for i in 0..100 {
            let scanner_clone = scanner.clone();
            handles.push(task::spawn(async move {
                let token = Address::new([i as u8; 20]);
                scanner_clone.scan(token, &[0x60; 100]).await
            }));
        }
        
        for h in handles {
            let _ = h.await;
        }
        
        let elapsed = start.elapsed();
        let (scans, threats) = scanner.get_stats().await;
        
        println!("[RACE] 100 concorrentes em {:?}", elapsed);
        println!("[STATS] Scans: {} | Threats: {}", scans, threats);
        
        assert_eq!(scans, 100, "Todos os scans devem completar");
        assert!(elapsed.as_millis() < 100, "100 scans devem ser < 100ms");
    }
}
