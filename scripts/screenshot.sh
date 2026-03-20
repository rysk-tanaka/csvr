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

if [[ "$(uname)" != "Darwin" ]]; then
  echo "error: this script requires macOS" >&2
  exit 1
fi

# Validate wait-time variables
# WAIT_LAUNCH must be an integer (used in arithmetic expansion)
if ! [[ "$WAIT_LAUNCH" =~ ^[0-9]+$ ]]; then
  echo "error: WAIT_LAUNCH must be a positive integer, got '$WAIT_LAUNCH'" >&2
  exit 1
fi
# WAIT_ACTION and WAIT_SHORT may be decimals (passed to sleep)
for var_name in WAIT_ACTION WAIT_SHORT; do
  val="${!var_name}"
  if ! [[ "$val" =~ ^[0-9]+\.?[0-9]*$ ]]; then
    echo "error: $var_name must be a non-negative number, got '$val'" >&2
    exit 1
  fi
done

# --- helpers ---

get_window_id() {
  local pid="$1"
  local stderr_file
  stderr_file=$(mktemp)
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
" 2>"$stderr_file"); then
    echo "error: swift failed. Is Xcode installed?" >&2
    echo "  output: $(cat "$stderr_file")" >&2
    rm -f "$stderr_file"
    return 1
  fi
  rm -f "$stderr_file"
  echo "$output"
}

# NOTE: $2 is interpolated into AppleScript; only pass trusted literals.
send_keys() {
  local pid="$1"
  local cmd="$2"
  local output
  if ! output=$(osascript -e "
    tell application \"System Events\"
      tell (first process whose unix id is $pid)
        set frontmost to true
        $cmd
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
    # Wait briefly, then force-kill if still alive
    for _ in 1 2 3 4 5; do
      kill -0 "$APP_PID" 2>/dev/null || return 0
      sleep 0.2
    done
    kill -9 "$APP_PID" 2>/dev/null || true
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
    if ! [[ "$WID" =~ ^[0-9]+$ ]]; then
      echo "error: get_window_id returned non-numeric value: '$WID'" >&2
      exit 1
    fi
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
send_keys "$APP_PID" 'keystroke "f" using command down'
sleep "$WAIT_SHORT"
send_keys "$APP_PID" 'keystroke "Tech"'
sleep "$WAIT_ACTION"
capture "$WID" "$OUTPUT_DIR/search.png"
# close search before chart capture
send_keys "$APP_PID" 'key code 53' # Escape
sleep "$WAIT_SHORT"

# 3) Chart — open with Cmd+G
echo "==> Capturing chart view..."
send_keys "$APP_PID" 'keystroke "g" using command down'
sleep "$WAIT_ACTION"
capture "$WID" "$OUTPUT_DIR/chart.png"

echo "==> Done! Screenshots saved to $OUTPUT_DIR/"
