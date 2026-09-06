# qdrop

Peer-to-peer clipboard, file, and link bridge for your own devices (macOS + Linux/Omarchy).

## Status

Milestone **M0** — project scaffold. The workspace builds two binaries that
share a core library; all CLI subcommands are present but stubbed until their
milestone lands (see [`MILESTONES.md`](MILESTONES.md)).

## Layout

| Crate | Kind | Role |
| --- | --- | --- |
| `crates/qdrop-core` | lib | config + peer registry + paths + logging |
| `crates/qdrop` | bin (`qdrop`) | command-line client |
| `crates/qdropd` | bin (`qdropd`) | long-running daemon |

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

## Configuration

Read from `~/.config/qdrop/` on both platforms (override the directory with
`QDROP_CONFIG_DIR`):

- `config.toml` — device name, port, sync toggles, limits. All fields optional.
- `peers.toml` — paired devices (populated by `qdrop pair`, M2).

Logs go to stderr. Raise verbosity with `-v`/`--verbose` or `RUST_LOG`.
