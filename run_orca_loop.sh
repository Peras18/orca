#!/bin/bash
cd ~/projects/ORCA
while true; do
    ts=$(date +%Y%m%d_%H%M%S)
    echo "=== [$(date)] A iniciar/reiniciar ORCA ==="
    ./run_orca.sh 2>&1 | tee -a "logs/run_vNN_${ts}.log"
    echo "=== [$(date)] ORCA terminou (watchdog ou crash) — reiniciando em 5s ==="
    sleep 5
done
