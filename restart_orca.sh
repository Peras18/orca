#!/bin/bash
# Restart automático se o bot crashar
LOG="/tmp/orca_watchdog.log"
LOCK="/tmp/orca_watchdog.lock"

# Impede mais de um watchdog vivo ao mesmo tempo (ex: vários @reboot
# sobrepostos depois de reinícios do WSL/Windows).
exec 9>"$LOCK"
if ! flock -n 9; then
    echo "[$(date)] Watchdog já em execução noutro processo -- a sair." >> "$LOG"
    exit 0
fi

echo "[$(date)] Watchdog iniciado (PID $$)" >> "$LOG"

while true; do
    RUNNING_COUNT=$(pgrep -x orca-engine | wc -l)

    if [ "$RUNNING_COUNT" -eq 0 ]; then
        echo "[$(date)] Bot não está a correr — a reiniciar..." >> "$LOG"
        cd ~/projects/ORCA
        gpg --quiet --decrypt .env.gpg > /tmp/.env_dec 2>/dev/null
        if [ $? -eq 0 ]; then
            set -a && source /tmp/.env_dec && set +a
            shred -zu /tmp/.env_dec
            nohup ./target/release/orca-engine >> /tmp/orca_live.log 2>&1 &
            echo "[$(date)] Bot reiniciado PID: $!" >> "$LOG"
        else
            echo "[$(date)] ERRO: falha na desencriptação" >> "$LOG"
        fi
    elif [ "$RUNNING_COUNT" -gt 1 ]; then
        # Nunca deveria acontecer com este watchdog corrigido, mas protege
        # contra lançamentos manuais concorrentes (ex: run_orca.sh à mão
        # enquanto o watchdog também decide reiniciar).
        echo "[$(date)] AVISO: ${RUNNING_COUNT} instâncias de orca-engine detectadas -- a manter só a mais antiga" >> "$LOG"
        pgrep -x orca-engine | sort -n | tail -n +2 | xargs -r kill -9
        echo "[$(date)] Instâncias extra terminadas." >> "$LOG"
    fi

    sleep 30
done
