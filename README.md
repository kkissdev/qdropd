# qdrop

Peer-to-peer clipboard, file, and link bridge for your own devices (macOS + Linux/Omarchy).

## Status

Milestone **M6** completes the daily-driver cut (M1-M6): clipboard **images**
sync too (PNG, inline or via the blob path when large), and password-manager
items (`org.nspasteboard.ConcealedType` / transient) are never synced. Plus
M5 link push, M4 file send, M3 clipboard text, M2 pinned-key TLS. Remaining
milestones are UI, packaging, and hardening (see [`MILESTONES.md`](MILESTONES.md)).

## Layout

| Crate | Kind | Role |
| --- | --- | --- |
| `crates/qdrop-core` | lib | config, peers/roster, identity + pinned TLS, PIN pairing, wire protocol, logging |
| `crates/qdrop` | bin (`qdrop`) | command-line client (`pair`, `peers`, …) |
| `crates/qdropd` | bin (`qdropd`) | daemon: discovery + authenticated transport |

## Pairing

```bash
# on device A
qdrop pair                 # prints a PIN, waits 60s
# on device B
qdrop pair alpha           # or: qdrop pair 192.168.1.50 — prompts for the PIN
```

Then start `qdropd` on both. `qdrop pair --remove <name>` unpairs.

## Build

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

## Try it

```bash
cargo run --bin qdrop -- --help
cargo run --bin qdropd -- --version
cargo run --bin qdropd -- --check   # load config, print the plan, exit
```

## Daemon

`qdropd` is a foreground shell command; see [`docs/qdropd.md`](docs/qdropd.md)
for full usage. A service-manager unit to run it in the background comes in M8.

## Configuration

Read from `~/.config/qdrop/` on both platforms (override the directory with
`QDROP_CONFIG_DIR`):

- `config.toml` — device name, port, sync toggles, limits. All fields optional.
- `peers.toml` — paired devices (populated by `qdrop pair`, M2).

Logs go to stderr. Raise verbosity with `-v`/`--verbose` or `RUST_LOG`.
