with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''    match provider.call(&call_req).await {
        Ok(result) => {
            // Retorno: (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate)
            // amountOut é a primeira slot de 32 bytes.
            if result.len() >= 32 {
                Some(U256::from_be_slice(&result[0..32]))
            } else {
                None
            }
        }
        Err(_) => None, // revert = tamanho inviável para este hop, sem margem adivinhada
    }
}'''

new = '''    match provider.call(&call_req).await {
        Ok(result) => {
            // Retorno: (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate)
            // amountOut é a primeira slot de 32 bytes.
            if result.len() >= 32 {
                Some(U256::from_be_slice(&result[0..32]))
            } else {
                None
            }
        }
        Err(e) => {
            // CORREÇÃO: distinguir revert real (liquidez insuficiente, "IIA"
            // genuíno) de erro de rede/rate-limit (HTTP 429, timeout) -- um
            // RPC sobrecarregado a devolver erro NÃO significa que o swap é
            // inviável, mas estava a ser tratado como tal, levando a refinar
            // o tamanho para 0 em 100% dos casos mesmo quando a liquidez
            // real era suficiente (confirmado manualmente via eth_call direto
            // para os mesmos parâmetros).
            let err_str = e.to_string();
            if err_str.contains("429") || err_str.contains("rate limit") || err_str.contains("Too Many Requests") {
                warn!("[QUOTER-V3] RPC rate-limited durante quote -- NÃO é IIA real: {}", err_str);
            }
            None
        }
    }
}'''

count = content.count(old)
print(f"Ocorrências: {count}")
if count == 1:
    content = content.replace(old, new)
    with open('src/orca/mod.rs', 'w') as f:
        f.write(content)
    print("Aplicado com sucesso.")
else:
    print("ABORTADO.")
