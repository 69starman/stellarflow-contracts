#!/bin/bash
set -e

MAX_SIZE=46080 # 45KB in bytes (45 * 1024)

# Find all wasm files in the target directory
# We exclude files in the build/debug/deps directories
wasm_files=$(find target/wasm32-unknown-unknown/release -maxdepth 1 -name "*.wasm")

for file in $wasm_files; do
    size=$(stat -c%s "$file")
    echo "Checking size of $file: $size bytes"
    if [ "$size" -gt "$MAX_SIZE" ]; then
        echo "Error: $file exceeds maximum size of 45KB (found $size bytes)"
        exit 1
    fi
done

echo "All WASM binaries are under 45KB."
