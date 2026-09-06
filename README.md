# qdrop

Peer-to-peer clipboard, file, and link bridge for your own devices (macOS + Linux/Omarchy).

## Status

Milestone **M1** — mDNS discovery + plaintext TCP transport. Two daemons on a
LAN find each other, hold one connection per pair, exchange a `Hello`
handshake, keepalive with `Ping`/`Pong`, and reconnect automatically. No
encryption yet (M2). CLI subcommands past `peers` are still stubbed (see
[`MILESTONES.md`](MILESTONES.md)).

## Layout

| Crate | Kind | Role |
| --- | --- | --- |
| `crates/qdrop-core` | lib | config, peer registry, paths, logging, wire protocol + framing, device id |
| `crates/qdrop` | bin (`qdrop`) | command-line client |
| `crates/qdropd` | bin (`qdropd`) | daemon: mDNS discovery + transport |

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
