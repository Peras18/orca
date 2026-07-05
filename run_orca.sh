#!/bin/bash
gpg --quiet --decrypt .env.gpg > /tmp/.env_dec 2>/dev/null
if [ $? -ne 0 ]; then echo "❌ Passphrase incorreta"; exit 1; fi
set -a && source /tmp/.env_dec && set +a
shred -zu /tmp/.env_dec
echo "✅ A iniciar ORCA..."
./target/release/orca-engine
