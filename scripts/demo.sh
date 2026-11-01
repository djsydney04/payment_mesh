#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -p mesh-server -p mesh-cli --locked
demo_directory=$(mktemp -d "${TMPDIR:-/tmp}/mesh-demo.XXXXXX")
port="${MESH_DEMO_PORT:-18787}"
MESH_BIND="127.0.0.1:$port" MESH_DATABASE="$demo_directory/demo.sqlite" MESH_MODE=sandbox \
  target/debug/mesh-server > "$demo_directory/server.log" 2>&1 &
server_pid=$!
cleanup() { kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; }
trap cleanup EXIT
ready=false
for attempt in $(seq 1 50); do
  if ! kill -0 "$server_pid" 2>/dev/null; then cat "$demo_directory/server.log"; exit 1; fi
  if curl --silent --fail "http://127.0.0.1:$port/health" >/dev/null; then ready=true; break; fi
  sleep 0.1
done
if [ "$ready" != true ]; then cat "$demo_directory/server.log"; exit 1; fi
target/debug/mesh-cli "http://127.0.0.1:$port"
