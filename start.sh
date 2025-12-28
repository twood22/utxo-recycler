#!/bin/bash
set -e

echo "Starting Tor..."
tor &

# Wait for Tor to be ready (check if SOCKS port is listening)
echo "Waiting for Tor to initialize..."
for i in {1..30}; do
    if nc -z 127.0.0.1 9050 2>/dev/null; then
        echo "Tor is ready!"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "Warning: Tor may not be fully ready, continuing anyway..."
    fi
    sleep 1
done

echo "Starting UTXO Recycler..."
exec /app/utxo-recycler
