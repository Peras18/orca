#!/bin/bash
cd /home/jgonp/projects/ORCA

LOG_DIR="logs"
MAX_LOG_MB=500
END=$((SECONDS + 50400))
DATE=$(date +%Y%m%d)

mkdir -p $LOG_DIR
# Nunca apaga o CSV — só cria se não existir
if [ ! -f "$LOG_DIR/opportunities_${DATE}.csv" ]; then
    echo "timestamp,block,path,hops,input_eth,gross_profit_eth,gas_cost_eth,net_profit_eth,net_profit_eur_1800" > $LOG_DIR/opportunities_${DATE}.csv
fi
# Symlink para o ficheiro de hoje
ln -sf opportunities_${DATE}.csv $LOG_DIR/opportunities.csv

echo "[WATCHDOG] Iniciado: $(date -u)" > $LOG_DIR/watchdog.log

cleanup_logs() {
    for f in $LOG_DIR/*.log $LOG_DIR/shadow_hunter.* $LOG_DIR/terminal.*; do
        [ -f "$f" ] || continue
        size_mb=$(du -sm "$f" 2>/dev/null | cut -f1)
        if [ "${size_mb:-0}" -gt "$MAX_LOG_MB" ]; then
            echo "[WATCHDOG] $(date -u) — $f > ${MAX_LOG_MB}MB, a truncar" >> $LOG_DIR/watchdog.log
            tail -c 50000000 "$f" > "$f.tmp" && mv "$f.tmp" "$f"
        fi
    done
}

while [ $SECONDS -lt $END ]; do
    echo "[WATCHDOG] $(date -u) — a lançar bot" >> $LOG_DIR/watchdog.log
    RUST_LOG=warn timeout 7200 ./target/release/orca-engine >> $LOG_DIR/dryrun_${DATE}.log 2>&1
    EXIT=$?
    echo "[WATCHDOG] $(date -u) — bot parou (exit=$EXIT)" >> $LOG_DIR/watchdog.log
    cleanup_logs
    [ $SECONDS -lt $END ] && sleep 5
done

echo "[WATCHDOG] $(date -u) — run completo" >> $LOG_DIR/watchdog.log
