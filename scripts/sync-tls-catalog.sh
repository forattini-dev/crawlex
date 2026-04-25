#!/usr/bin/env bash
# Bulk-capture TLS ClientHello fingerprints across Chrome/Chromium/Firefox
# major versions and emit curl-impersonate-style YAML signatures into
# src/impersonate/catalog/captured/.
#
# Idempotent: skips downloads that already exist in
# ~/.cache/crawlex/browsers/. Re-run after a major hash mismatch from the
# mining oracle to refresh.
#
# Pipeline per browser:
#   1. Resolve download URL (Chrome / Chromium / Firefox)
#   2. Download + extract to cache dir (skip if cached)
#   3. Start tls-canary on 127.0.0.1:8443 in background
#   4. Launch headless browser pointed at https://127.0.0.1:8443
#      with --ignore-certificate-errors
#   5. Wait for tls-canary to write the .bin
#   6. Kill browser + canary
#   7. Run yaml-from-capture.mjs to produce YAML
#   8. Cross-check JA3 against mined oracle (warn on mismatch)
#
# Total: 30 Chrome + 30 Chromium + 20 Firefox = 80 captures.
# Estimated runtime: ~4-6h depending on network bandwidth.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE_DIR="${CRAWLEX_BROWSER_CACHE:-$HOME/.cache/crawlex/browsers}"
CAPTURE_DIR="${CRAWLEX_TLS_CAPTURE_DIR:-/tmp/crawlex-tls-captures}"
OUT_DIR="$REPO_ROOT/src/impersonate/catalog/captured"
MINED_DIR="$REPO_ROOT/src/impersonate/catalog/mined"
CANARY_BIN="$REPO_ROOT/target/debug/tls-canary"
CANARY_LISTEN="127.0.0.1:8443"

# Range knobs — override via env to subset.
CHROME_MAJORS=(${CHROME_MAJORS:-$(seq 120 149)})
CHROMIUM_MAJORS=(${CHROMIUM_MAJORS:-$(seq 120 149)})
FIREFOX_MAJORS=(${FIREFOX_MAJORS:-$(seq 111 130)})

mkdir -p "$CACHE_DIR" "$CAPTURE_DIR" "$OUT_DIR"

build_canary() {
  if [[ ! -x "$CANARY_BIN" ]]; then
    echo "==> building tls-canary"
    rustc "$REPO_ROOT/scripts/tls-canary.rs" -o "$CANARY_BIN"
  fi
}

start_canary() {
  local label="$1"
  rm -f "$CAPTURE_DIR"/${label}_*.bin "$CAPTURE_DIR"/${label}_*.meta.json
  "$CANARY_BIN" --listen "$CANARY_LISTEN" --out "$CAPTURE_DIR" --label "$label" &
  CANARY_PID=$!
  sleep 0.3
}

stop_canary() {
  if [[ -n "${CANARY_PID:-}" ]] && kill -0 "$CANARY_PID" 2>/dev/null; then
    kill "$CANARY_PID" 2>/dev/null || true
    wait "$CANARY_PID" 2>/dev/null || true
  fi
  CANARY_PID=
}

trap stop_canary EXIT

wait_for_capture() {
  local label="$1"
  local deadline=$((SECONDS + 15))
  while (( SECONDS < deadline )); do
    if ls "$CAPTURE_DIR"/${label}_*.bin >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done
  return 1
}

capture_chrome_for_testing() {
  local major="$1"
  local label="chrome_${major}_linux"

  # Chrome for Testing manifest gives us "Last Known Good" patch numbers.
  local manifest_url="https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json"
  local manifest="$CACHE_DIR/cft-manifest.json"
  if [[ ! -f "$manifest" ]] || (( $(date +%s) - $(stat -c %Y "$manifest" 2>/dev/null || echo 0) > 86400 )); then
    echo "==> refreshing CFT manifest"
    curl -sSL "$manifest_url" -o "$manifest"
  fi

  # Pick first matching version for the major.
  local full_version
  full_version=$(jq -r --arg M "$major" \
    '.versions | map(select(.version | startswith($M + "."))) | last | .version' "$manifest")
  if [[ -z "$full_version" || "$full_version" == "null" ]]; then
    echo "  !! no CFT release for chrome major $major — skipping"
    return 1
  fi

  local zip_url
  zip_url=$(jq -r --arg V "$full_version" \
    '.versions[] | select(.version == $V) | .downloads.chrome[] | select(.platform == "linux64") | .url' \
    "$manifest")
  local install_dir="$CACHE_DIR/chrome-$full_version"
  if [[ ! -d "$install_dir" ]]; then
    echo "==> downloading chrome $full_version"
    mkdir -p "$install_dir"
    curl -sSL "$zip_url" -o "$install_dir/chrome.zip"
    (cd "$install_dir" && unzip -q chrome.zip)
  fi
  local chrome_bin="$install_dir/chrome-linux64/chrome"

  echo "==> capturing chrome $full_version → $label"
  start_canary "$label"
  "$chrome_bin" \
    --headless --disable-gpu \
    --ignore-certificate-errors \
    --user-data-dir="$(mktemp -d)" \
    --no-sandbox \
    "https://${CANARY_LISTEN}/" >/dev/null 2>&1 &
  local browser_pid=$!
  if wait_for_capture "$label"; then
    kill "$browser_pid" 2>/dev/null || true
    stop_canary
    local bin_file
    bin_file=$(ls -t "$CAPTURE_DIR"/${label}_*.bin | head -1)
    node "$REPO_ROOT/scripts/yaml-from-capture.mjs" \
      --bin "$bin_file" --browser chrome --version "$full_version" --os linux \
      --out "$OUT_DIR/chrome_${full_version}_linux.yaml" \
      --mined-oracles "$MINED_DIR" || true
  else
    echo "  !! capture timed out for chrome $full_version"
    kill "$browser_pid" 2>/dev/null || true
    stop_canary
  fi
}

capture_chromium_snapshot() {
  local major="$1"
  local label="chromium_${major}_linux"
  # Chromium snapshots are keyed by build position (e.g. 1234567), not majors.
  # We use chromiumdash to find the build position for a given major.
  local position_url="https://chromiumdash.appspot.com/fetch_milestones?mstone=${major}&platform=Linux"
  local position
  position=$(curl -sSL "$position_url" | jq -r '.[0].chromium_main_branch_position')
  if [[ -z "$position" || "$position" == "null" ]]; then
    echo "  !! no chromium snapshot for major $major"
    return 1
  fi
  local install_dir="$CACHE_DIR/chromium-$position"
  if [[ ! -d "$install_dir" ]]; then
    echo "==> downloading chromium snapshot $position (for chrome $major)"
    mkdir -p "$install_dir"
    curl -sSL "https://commondatastorage.googleapis.com/chromium-browser-snapshots/Linux_x64/${position}/chrome-linux.zip" \
      -o "$install_dir/chromium.zip"
    (cd "$install_dir" && unzip -q chromium.zip)
  fi
  local chromium_bin="$install_dir/chrome-linux/chrome"
  if [[ ! -x "$chromium_bin" ]]; then
    echo "  !! chromium binary missing in $install_dir"
    return 1
  fi

  echo "==> capturing chromium snapshot $position → $label"
  start_canary "$label"
  "$chromium_bin" \
    --headless --disable-gpu \
    --ignore-certificate-errors \
    --user-data-dir="$(mktemp -d)" \
    --no-sandbox \
    "https://${CANARY_LISTEN}/" >/dev/null 2>&1 &
  local browser_pid=$!
  if wait_for_capture "$label"; then
    kill "$browser_pid" 2>/dev/null || true
    stop_canary
    local bin_file
    bin_file=$(ls -t "$CAPTURE_DIR"/${label}_*.bin | head -1)
    node "$REPO_ROOT/scripts/yaml-from-capture.mjs" \
      --bin "$bin_file" --browser chromium --version "${major}.0.${position}.0" --os linux \
      --out "$OUT_DIR/chromium_${major}.0.${position}.0_linux.yaml" \
      --mined-oracles "$MINED_DIR" || true
  else
    echo "  !! capture timed out for chromium $position"
    kill "$browser_pid" 2>/dev/null || true
    stop_canary
  fi
}

capture_firefox() {
  local major="$1"
  local label="firefox_${major}_linux"
  local minor="0"
  local version="${major}.${minor}"
  local archive_url="https://archive.mozilla.org/pub/firefox/releases/${version}/linux-x86_64/en-US/firefox-${version}.tar.bz2"
  local install_dir="$CACHE_DIR/firefox-$version"
  if [[ ! -d "$install_dir" ]]; then
    echo "==> downloading firefox $version"
    mkdir -p "$install_dir"
    if ! curl -sSLf "$archive_url" -o "$install_dir/firefox.tar.bz2"; then
      echo "  !! firefox $version not found at $archive_url"
      rm -rf "$install_dir"
      return 1
    fi
    (cd "$install_dir" && tar -xjf firefox.tar.bz2)
  fi
  local firefox_bin="$install_dir/firefox/firefox"
  if [[ ! -x "$firefox_bin" ]]; then
    echo "  !! firefox binary missing"
    return 1
  fi

  echo "==> capturing firefox $version → $label"
  start_canary "$label"
  # Firefox needs a profile dir; use ephemeral.
  local profile_dir
  profile_dir=$(mktemp -d)
  "$firefox_bin" \
    --headless \
    --no-remote \
    --profile "$profile_dir" \
    "https://${CANARY_LISTEN}/" >/dev/null 2>&1 &
  local browser_pid=$!
  if wait_for_capture "$label"; then
    kill "$browser_pid" 2>/dev/null || true
    stop_canary
    local bin_file
    bin_file=$(ls -t "$CAPTURE_DIR"/${label}_*.bin | head -1)
    node "$REPO_ROOT/scripts/yaml-from-capture.mjs" \
      --bin "$bin_file" --browser firefox --version "$version" --os linux \
      --out "$OUT_DIR/firefox_${version}_linux.yaml" \
      --mined-oracles "$MINED_DIR" || true
  else
    echo "  !! capture timed out for firefox $version"
    kill "$browser_pid" 2>/dev/null || true
    stop_canary
  fi
  rm -rf "$profile_dir"
}

main() {
  build_canary
  echo "==> starting bulk capture"
  echo "    chrome    : ${CHROME_MAJORS[*]}"
  echo "    chromium  : ${CHROMIUM_MAJORS[*]}"
  echo "    firefox   : ${FIREFOX_MAJORS[*]}"
  echo "    cache     : $CACHE_DIR"
  echo "    output    : $OUT_DIR"

  for major in "${CHROME_MAJORS[@]}"; do
    capture_chrome_for_testing "$major" || true
  done
  for major in "${CHROMIUM_MAJORS[@]}"; do
    capture_chromium_snapshot "$major" || true
  done
  for major in "${FIREFOX_MAJORS[@]}"; do
    capture_firefox "$major" || true
  done

  echo "==> done. captured YAMLs in $OUT_DIR"
}

main "$@"
