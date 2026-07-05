with open('src/orca/mod.rs', 'r') as f:
    content = f.read()

old = '''                None => {
                    info!(
                        "[DIAG-CYCLE] hop {} (V3) FALHOU: token_in={:?} token_out={:?} fee={} amount_in={}",
                        hop_idx, hop.token_in, hop.token_out, hop.fee, amount
                    );
                    return None;
                }'''

new = '''                None => {
                    info!(
                        "[DIAG-CYCLE] hop {} (V3) FALHOU: pool={:?} token_in={:?} token_out={:?} fee={} amount_in={} liquidity_no_cache={:?}",
                        hop_idx, hop.pool, hop.token_in, hop.token_out, hop.fee, amount, hop.liquidity
                    );
                    return None;
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
