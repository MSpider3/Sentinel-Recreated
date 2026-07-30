#!/usr/bin/env bash
set -e

echo "=== Sentinel Daemon Cold-Start Benchmark ==="

if [ "$EUID" -ne 0 ]; then
    echo "Error: Must be run as root (sudo ./tests/bench_coldstart.sh)"
    exit 1
fi

echo "Stopping sentinel service..."
systemctl stop sentinel
sleep 1

echo "Measuring cold-start duration..."
START=$(date +%s%N)
systemctl start sentinel

MAX_WAIT=60  # 6 seconds max (60 x 0.1s)
COUNT=0
while [ $COUNT -lt $MAX_WAIT ]; do
    busctl call com.sentinel.Sentinel /com/sentinel/Sentinel \
        com.sentinel.Sentinel GetStatus &>/dev/null && break
    sleep 0.1
    COUNT=$((COUNT + 1))
done

END=$(date +%s%N)

if [ $COUNT -eq $MAX_WAIT ]; then
    echo "TIMEOUT: Daemon did not start within 6s (6000ms)"
    exit 1
fi

DURATION_MS=$(( (END - START) / 1000000 ))
echo "Cold start time: ${DURATION_MS}ms"

if [ "$DURATION_MS" -le 5000 ]; then
    echo ">>> RESULT: PASS (Cold start ${DURATION_MS}ms <= 5000ms target) <<<"
else
    echo ">>> RESULT: WARNING (Cold start ${DURATION_MS}ms > 5000ms target) <<<"
fi
