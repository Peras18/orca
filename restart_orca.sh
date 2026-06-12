#!/bin/bash
# Restart automático se o bot crashar
LOG="/tmp/orca_watchdog.log"
echo "[$(date)] Watchdog iniciado" >> $LOG
while true; do
    if ! pgrep -x orca-engine > /dev/null; then
        echo "[$(date)] Bot não está a correr — a reiniciar..." >> $LOG
        cd ~/projects/ORCA
        gpg --quiet --decrypt .env.gpg > /tmp/.env_dec 2>/dev/null
        if [ $? -eq 0 ]; then
            set -a && source /tmp/.env_dec && set +a
            shred -zu /tmp/.env_dec
            nohup ./target/release/orca-engine >> /tmp/orca_live.log 2>&1 &
            echo "[$(date)] Bot reiniciado PID: $!" >> $LOG
        else
            echo "[$(date)] ERRO: falha na desencriptação" >> $LOG
        fi
    fi
    sleep 30
done
