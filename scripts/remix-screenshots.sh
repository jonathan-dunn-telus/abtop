#!/usr/bin/env bash
# Regenerate the README screenshots in assets/remix/ from `abtop --demo`.
#
# Only demo mode is captured: its sessions are synthetic, so no real project,
# prompt, or path can leak into a published image. A throwaway HOME keeps the
# capture independent of (and from writing to) your own abtop config.
#
# Requires: tmux, python3, a headless Chromium (CHROME, or Playwright's
# chrome-headless-shell), and the DejaVu fonts (FONT_DIR). scripts/ansi_to_html.py
# lays the capture out on a fixed cell grid, because no common monospace font
# has braille glyphs and fallback glyphs would otherwise skew the chart rows.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/assets/remix"
fonts="${FONT_DIR:-/usr/share/fonts/truetype/dejavu}"
chrome="${CHROME:-$(ls -d "$HOME"/.cache/ms-playwright/chromium_headless_shell-*/*/chrome-headless-shell 2>/dev/null | tail -1)}"
socket="abtop-shots-$$"
work="$(mktemp -d)"
trap 'tmux -L "$socket" kill-server 2>/dev/null || true; rm -rf "$work"' EXIT

cargo build --release --manifest-path "$root/Cargo.toml" --quiet
mkdir -p "$out"
cat >"$work/tmux.conf" <<'EOF'
set -g default-terminal "tmux-256color"
set -g terminal-features "*:RGB"
set -g status off
EOF

# capture <name> <cols> <rows> [keys...]
capture() {
  local name="$1" cols="$2" rows="$3"
  shift 3
  # Keys like C, a, and panel toggles are saved to config: isolate each capture.
  local home="$work/home-$name"
  mkdir -p "$home"
  tmux -L "$socket" -f "$work/tmux.conf" new-session -d -s shot -x "$cols" -y "$rows" \
    env HOME="$home" XDG_CONFIG_HOME="$home/.config" LANG=en_US.UTF-8 \
    COLORTERM=truecolor "$root/target/release/abtop" --demo
  # Let the demo replay a few refreshes so rates and charts move.
  sleep 5
  for key in "$@"; do
    tmux -L "$socket" send-keys -t shot "$key"
    sleep 0.5
  done
  sleep 2
  tmux -L "$socket" capture-pane -t shot -e -p >"$work/$name.ansi"
  tmux -L "$socket" kill-server
  local size
  size="$(python3 -I "$root/scripts/ansi_to_html.py" "$work/$name.ansi" "$work/$name.html" "$fonts")"
  "$chrome" --no-sandbox --allow-file-access-from-files --hide-scrollbars \
    --force-device-scale-factor=2 --window-size="$size" \
    --screenshot="$out/$name.png" "file://$work/$name.html" </dev/null >/dev/null 2>&1
  echo "wrote assets/remix/$name.png"
}

capture overview 180 52 C
capture all-live 180 52 a
capture narrow-usage 100 44 u
# 2 hides Quota, 5 hides Ports; C adds the cache chart, a selects All live.
capture focused 180 52 2 5 C a
