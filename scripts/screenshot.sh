#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_DIR/target/release/csvr"
SAMPLE_CSV="$PROJECT_DIR/examples/sample.csv"
OUTPUT_DIR="$PROJECT_DIR/docs/images"
WAIT_LAUNCH="${WAIT_LAUNCH:-10}"
WAIT_ACTION="${WAIT_ACTION:-1}"
WAIT_SHORT="${WAIT_SHORT:-0.5}"

# --- helpers ---

get_window_id() {
  local pid="$1"
  local output
  if ! output=$(swift -e "
import CoreGraphics
let targetPID = Int32($pid)
if let list = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] {
    for w in list {
        if let ownerPID = w[\"kCGWindowOwnerPID\"] as? Int32, ownerPID == targetPID {
            if let wid = w[\"kCGWindowNumber\"] as? Int {
                print(wid)
            }
            break
        }
    }
}
" 2>&1); then
    echo "error: swift failed. Is Xcode installed?" >&2
    echo "  output: $output" >&2
    return 1
  fi
  echo "$output"
}

# NOTE: $1 is interpolated into AppleScript; only pass trusted literals.
send_keys() {
  local output
  if ! output=$(osascript -e "
    tell application \"System Events\"
      tell (first process whose unix id is $APP_PID)
        set frontmost to true
        $1
      end tell
    end tell
  " 2>&1); then
    echo "error: osascript failed (Accessibility permission may be missing)" >&2
    echo "  output: $output" >&2
    exit 1
  fi
}

capture() {
  local wid="$1"
  local output="$2"
  screencapture -l "$wid" -o "$output"
  if [[ ! -s "$output" ]]; then
    echo "error: screencapture produced empty file: $output" >&2
    echo "  (Screen Recording permission may be missing)" >&2
    exit 1
  fi
  echo "  saved: $output"
}

cleanup() {
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# --- main ---

echo "==> Building release binary..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" --quiet

if [[ ! -f "$SAMPLE_CSV" ]]; then
  echo "error: $SAMPLE_CSV not found" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

echo "==> Launching csvr..."
"$BINARY" "$SAMPLE_CSV" &
APP_PID=$!

# Poll until the window appears or timeout
deadline=$(( $(date +%s) + WAIT_LAUNCH ))
WID=""
while :; do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "error: csvr (PID $APP_PID) crashed during launch" >&2
    exit 1
  fi
  WID=$(get_window_id "$APP_PID") || {
    echo "error: failed to query window list (swift error)" >&2
    exit 1
  }
  if [[ -n "$WID" ]]; then
    break
  fi
  if (( $(date +%s) >= deadline )); then
    echo "error: csvr window not found within ${WAIT_LAUNCH}s" >&2
    exit 1
  fi
  sleep "$WAIT_SHORT"
done
echo "  window id: $WID"

# 1) Table view
echo "==> Capturing table view..."
capture "$WID" "$OUTPUT_DIR/table.png"

# 2) Search — open with Cmd+F, type filter text
echo "==> Capturing search view..."
send_keys 'keystroke "f" using command down'
sleep "$WAIT_SHORT"
send_keys 'keystroke "Tech"'
sleep "$WAIT_ACTION"
capture "$WID" "$OUTPUT_DIR/search.png"
# close search before chart capture
send_keys 'key code 53' # Escape
sleep "$WAIT_SHORT"

# 3) Chart — open with Cmd+G
echo "==> Capturing chart view..."
send_keys 'keystroke "g" using command down'
sleep "$WAIT_ACTION"
capture "$WID" "$OUTPUT_DIR/chart.png"

echo "==> Done! Screenshots saved to $OUTPUT_DIR/"
