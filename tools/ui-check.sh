#!/usr/bin/env bash
# Fuzz the UI against a throwaway, freshly seeded server.
#
#   tools/ui-check.sh [seconds]      # default 60s per page
#
# Env: BOMBADIL (default: bombadil), CHROME (chromium binary if it is not in a
# standard location), PORT (default 19418), MICROTEL (default target/debug/microtel).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
seconds=${1:-60}
port=${PORT:-19418}
origin="http://127.0.0.1:$port"
bombadil=${BOMBADIL:-bombadil}
microtel=${MICROTEL:-$root/target/debug/microtel}
work=$(mktemp -d "${TMPDIR:-/tmp}/microtel-ui-check.XXXXXX")
out=${OUT:-"$root/.tmp/bombadil-$(date +%Y%m%d-%H%M%S)-$$"}

if ! command -v "$bombadil" >/dev/null; then
	if [ -x "$root/tools/node_modules/.bin/bombadil" ] && [ "$bombadil" = bombadil ]; then
		bombadil="$root/tools/node_modules/.bin/bombadil"
	else
		echo "bombadil not found ($bombadil); install tools dependencies or set BOMBADIL" >&2
		exit 2
	fi
fi
[ -x "$microtel" ] || { echo "build microtel first ($microtel)" >&2; exit 2; }

cleanup() {
	[ -n "${server:-}" ] && kill "$server" 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT

"$microtel" --listen "127.0.0.1:$port" --storage-dir "$work/data" >"$work/server.log" 2>&1 &
server=$!
for _ in $(seq 1 100); do
	curl -fsS "$origin/health" >/dev/null 2>&1 && break
	sleep 0.1
done

(cd "$root/tools" && EMIT_ENDPOINT="$origin" node seed.mjs >/dev/null)

# wait for the ingest queue to flush before asking for an exemplar
trace=""
for _ in $(seq 1 50); do
	trace=$(curl -fsS "$origin/api/spans/barchart?from=$(( $(date +%s%3N) - 3600000 ))&to=$(( $(date +%s%3N) + 60000 ))&group_by=service" |
		node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const e=JSON.parse(s).exemplars?.[0];if(e)console.log(`${e.trace_id} ${e.t}`)})')
	[ -n "$trace" ] && break
	sleep 0.2
done
[ -n "$trace" ] || { echo "no exemplars after seeding" >&2; exit 1; }
set -- $trace

run() {
	local name=$1 url=$2
	echo "== $name: $url"
	"$bombadil" browser test --headless --no-sandbox --exit-on-violation \
		--time-limit="${seconds}s" --output-path "$out/$name" \
		"$url" "$root/tools/bombadil/spec.ts"
}

status=0
run spans "$origin/spans" || status=1
run metrics "$origin/metrics" || status=1
run trace "$origin/trace/$1?at=$2" || status=1

if [ "$status" -eq 0 ]; then
	echo "ui check passed (${seconds}s per page); traces in $out"
else
	echo "ui check found violations; inspect with: bombadil browser inspect $out"
fi
exit "$status"
