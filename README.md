# qdrop

Peer-to-peer clipboard, file, and link bridge for your own devices (macOS + Linux/Omarchy).
End-to-end encrypted (TLS 1.3, keys pinned at pairing), LAN-only, no account, no relay.
All nine milestones from [`MILESTONES.md`](MILESTONES.md) are implemented; see
[`docs/threat-model.md`](docs/threat-model.md).

## 60-second setup

```bash
# on both machines
git clone https://github.com/kkissdev/qdropd && cd qdropd
./packaging/install.sh          # builds, installs qdrop + qdropd, starts the service

# pair them (once)
qdrop pair                      # machine A: prints a 6-digit PIN
qdrop pair <A-name-or-ip>       # machine B: type the PIN
```

That's it — copy text/images on one machine and they appear on the other;
`qdrop send file.pdf`, `qdrop open https://…`. Reboot both, it reconnects on
its own.

## Commands

| Command | Does |
| --- | --- |
| `qdrop pair [<name\|ip>]` | pair with a device (`--remove <name>` to unpair) |
| `qdrop peers` | list paired devices + online/last-seen |
| `qdrop status [--json\|--waybar]` | live daemon status |
| `qdrop send <path>… [--to <name>]` | send files to a peer |
| `qdrop open <url> [--to <name>]` | open a URL on a peer |
| `qdrop clip --pause\|--resume\|--toggle\|--status` | control clipboard sync |
| `qdrop auth [<name>]` | pre-authorize a peer to send unattended (`--remove` / `--mutual`) |
| `qdrop recv [--stdout]` | wait for an incoming file; print its path or stream it |
| `qdrop paste` / `qdrop copy` | read / set the shared clipboard from a pipe |
| `qdrop clip --history\|--restore N\|--send N` | recent clipboard entries |
| `qdrop doctor [--json]` | diagnose why sync isn't working |
| `qdrop daemon` / `qdropd` | run the daemon in the foreground |

## Layout

| Crate | Role |
| --- | --- |
| `crates/qdrop-core` | config, peers/roster, identity + pinned TLS, PIN pairing, wire protocol |
| `crates/qdrop` | the `qdrop` CLI |
| `crates/qdropd` | the daemon: discovery, transport, clipboard sync, file transfer |

`packaging/` has the launchd / systemd units, a Homebrew formula, a PKGBUILD,
`install.sh`, and generated man pages + shell completions
(`cargo run -p qdrop --example gen-artifacts`). `contrib/` has the macOS
menu-bar shell, the waybar module, and send-trigger recipes.

## Configuration

`~/.config/qdrop/` on both platforms (`QDROP_CONFIG_DIR` overrides):
`config.toml` (all keys optional), `peers.toml` (pairing writes it),
`identity.pem` (0600), `state.json` (daemon-written). See
[`docs/qdropd.md`](docs/qdropd.md).

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```
