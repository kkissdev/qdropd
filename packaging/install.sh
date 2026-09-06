#!/usr/bin/env bash
# qdrop one-command install: build, install the binaries, install and start the
# per-user service. Re-run any time to upgrade.
#
#   ./packaging/install.sh
#
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

command -v cargo >/dev/null || { echo "error: cargo (Rust) is required"; exit 1; }

echo "==> Building release binaries"
cargo build --release --workspace --bin qdrop --bin qdropd

os="$(uname -s)"
if [ "$os" = "Darwin" ]; then
  bindir="/usr/local/bin"
  [ -w "$bindir" ] || bindir="$HOME/.local/bin"
else
  bindir="$HOME/.local/bin"
fi
mkdir -p "$bindir"

echo "==> Installing qdrop, qdropd to $bindir"
install -m755 target/release/qdrop "$bindir/qdrop"
install -m755 target/release/qdropd "$bindir/qdropd"

case ":$PATH:" in
  *":$bindir:"*) ;;
  *) echo "note: add $bindir to your PATH" ;;
esac

if [ "$os" = "Darwin" ]; then
  plist="$HOME/Library/LaunchAgents/com.sysrex.qdropd.plist"
  echo "==> Installing LaunchAgent -> $plist"
  sed "s#/usr/local/bin/qdropd#$bindir/qdropd#" \
    packaging/launchd/com.sysrex.qdropd.plist > "$plist"
  launchctl unload "$plist" 2>/dev/null || true
  launchctl load "$plist"
  echo "==> qdropd is running (launchctl list | grep qdropd)"
else
  unit_dir="$HOME/.config/systemd/user"
  mkdir -p "$unit_dir"
  echo "==> Installing systemd --user unit -> $unit_dir/qdropd.service"
  sed "s#%h/.local/bin/qdropd#$bindir/qdropd#" \
    packaging/systemd/qdropd.service > "$unit_dir/qdropd.service"
  systemctl --user daemon-reload
  systemctl --user enable --now qdropd.service
  echo "==> qdropd is running (systemctl --user status qdropd)"
fi

echo
echo "Done. Next: pair with your other device —"
echo "    qdrop pair                 # on this machine, shows a PIN"
echo "    qdrop pair <name-or-ip>    # on the other machine"
