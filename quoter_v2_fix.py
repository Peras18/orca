with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''const MIN_FLASH_WEI_U256: U256 = U256::from_limbs([10_000_000_000_000_000u64, 0, 0, 0]);'''

new = '''const MIN_FLASH_WEI_U256: U256 = U256::from_limbs([10_000_000_000_000_000u64, 0, 0, 0]);

/// Endereço oficial do QuoterV2 da Uniswap V3 na Base Mainnet.
/// Confirmado contra docs.uniswap.org/contracts/v3/reference/deployments/base-deployments
/// e validado on-chain: factory() devolve 0x33128a8fC17869897dcE68Ed026d694621f6FDfD,
/// que é a UniswapV3Factory real na Base (a nossa constante UniswapV3Factory::ADDRESS
/// estava errada -- corrigida em src/contracts/uniswap_v3.rs).
const UNISWAP_V3_QUOTER_V2: Address = Address::new([
    0x3d, 0x4e, 0x44, 0xEb, 0x13, 0x74, 0x24, 0x0C, 0xE5, 0xF1,
    0xB8, 0x71, 0xab, 0x26, 0x1C, 0xD1, 0x63, 0x35, 0xB7, 0x6a,
]);

/// 🎯 CORREÇÃO DE CAUSA RAIZ (erro "IIA" / Insufficient Input Amount):
///
/// simulate_cycle_profit_wei() usa a fórmula de produto constante (x*y=k,
/// fee 0.3% fixo) -- válida para AMMs V2-style com liquidez uniforme, mas
/// estruturalmente ERRADA para Uniswap V3, que tem liquidez concentrada em
/// ticks discretos. Um hop V3 grande pode atravessar múltiplos ticks com
/// liquidez muito diferente entre eles -- a fórmula V2 ignora isso por
/// completo e sobrestima sistematicamente quanto pode ser trocado sem
/// reverter, causando "IIA" no eth_call real.
///
/// Esta função substitui a aproximação por uma simulação EXATA, usando o
/// QuoterV2 oficial da própria Uniswap (gratuito via eth_call, sem gastar
/// gás real -- é justamente para isto que o protocolo o disponibiliza).
/// Devolve None se a simulação reverter (ex: liquidez insuficiente mesmo
/// para o tick atual) -- nesse caso o hop não é viável para este tamanho,
/// ponto final, sem adivinhar margens de segurança arbitrárias.
async fn quote_v3_exact_input(
    provider: &impl AlloyProvider,
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
) -> Option<U256> {
    use alloy::rpc::types::TransactionRequest;
    use alloy::network::TransactionBuilder;

    // quoteExactInputSingle((address tokenIn, address tokenOut, uint256 amountIn, uint24 fee, uint160 sqrtPriceLimitX96))
    // selector: 0xc6a5026a
    let mut calldata: Vec<u8> = Vec::with_capacity(4 + 32 * 5);
    calldata.extend_from_slice(&[0xc6, 0xa5, 0x02, 0x6a]);
    // struct é passada inline (não é dynamic type, é tuple simples -- cada
    // campo ocupa exactamente uma slot de 32 bytes, sem offset/length).
    let mut token_in_padded = [0u8; 32];
    token_in_padded[12..].copy_from_slice(token_in.as_slice());
    calldata.extend_from_slice(&token_in_padded);

    let mut token_out_padded = [0u8; 32];
    token_out_padded[12..].copy_from_slice(token_out.as_slice());
    calldata.extend_from_slice(&token_out_padded);

    calldata.extend_from_slice(&amount_in.to_be_bytes::<32>());
    calldata.extend_from_slice(&U256::from(fee).to_be_bytes::<32>());
    calldata.extend_from_slice(&U256::ZERO.to_be_bytes::<32>()); // sqrtPriceLimitX96 = 0 (sem limite)

    let call_req = TransactionRequest::default()
        .with_to(UNISWAP_V3_QUOTER_V2)
        .with_input(alloy::primitives::Bytes::from(calldata));

    match provider.call(&call_req).await {
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
}

/// Encontra, via busca binária real contra o QuoterV2 (não aproximação),
/// o maior amount_in para um hop V3 que ainda produz um quote válido
/// (não reverte). Usa no máximo ~10 chamadas eth_call (gratuitas, ~50ms
/// cada em paralelo na prática) -- converge rápido porque é busca binária
/// sobre um espaço já estreitado pelo optimal_cycle_input V2 como ponto de
/// partida (max_candidate), não desde zero.
async fn find_max_viable_v3_input(
    provider: &impl AlloyProvider,
    token_in: Address,
    token_out: Address,
    fee: u32,
    max_candidate: U256,
) -> U256 {
    if max_candidate.is_zero() {
        return U256::ZERO;
    }
    // Primeiro: o candidato máximo já funciona? Caso comum quando a
    // liquidez é suficiente -- evita busca binária desnecessária.
    if quote_v3_exact_input(provider, token_in, token_out, fee, max_candidate).await.is_some() {
        return max_candidate;
    }
    let mut lo = U256::ZERO;
    let mut hi = max_candidate;
    for _ in 0..10 {
        if hi <= lo {
            break;
        }
        let mid = lo + (hi - lo) / U256::from(2u64);
        if mid.is_zero() {
            break;
        }
        if quote_v3_exact_input(provider, token_in, token_out, fee, mid).await.is_some() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
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
