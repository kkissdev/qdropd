# `qdropd` — the qdrop daemon

`qdropd` is a **command-line program**. You run it from a shell. It is a
long-running foreground process: it stays attached to the terminal, logs to
stderr, and exits on `Ctrl-C` or `SIGTERM`. In production a service manager
(`launchd` on macOS, `systemd --user` on Linux) keeps it running in the
background — that packaging arrives in M8. Until then you start it by hand.

```bash
qdropd
```

`qdrop` (no `d`) is the separate **control CLI** you use day to day — `qdrop
send`, `qdrop peers`, `qdrop pair`. `qdropd` is only the background engine.

---

## Running from a build

There is no installed binary yet. From a clone of the repo:

```bash
cargo run --bin qdropd            # debug build, foreground
cargo run --bin qdropd -- --check # load + validate config, then exit
```

Or build once and call the binary directly:

```bash
cargo build --release
./target/release/qdropd --version
./target/release/qdropd
```

---

## Usage

```
qdropd [OPTIONS]
```

| Option | Description |
| --- | --- |
| `-v`, `--verbose` | Debug-level logging. Overridden by `RUST_LOG`. |
| `--port <PORT>` | Override the listen port from `config.toml`. |
| `--check` | Load config, log what it would do, exit `0`. Nothing is bound. |
| `-V`, `--version` | Print version and exit. |
| `-h`, `--help` | Print help and exit. |

Exit code is `0` on clean shutdown, non-zero if config fails to load or the
listen port is unavailable.

### Examples

```bash
# Normal run
qdropd

# Verbose, on a non-default port
qdropd --verbose --port 51000

# CI / packaging smoke check — never touches the network
qdropd --check

# Crank up logging for one subsystem only
RUST_LOG=qdropd::transport=debug,info qdropd
```

---

## Configuration

`qdropd` and `qdrop` share config under `~/.config/qdrop/` on **both** macOS
and Linux. Set `QDROP_CONFIG_DIR` to point elsewhere (used by tests).

- **`config.toml`** — every key is optional; omitted keys take their default.

  | Key | Default | Meaning |
  | --- | --- | --- |
  | `device_name` | system hostname | Name shown to peers |
  | `port` | `47654` | TCP listen port |
  | `sync_clipboard` | `true` | Master clipboard-sync switch |
  | `sync_images` | `true` | Include images in clipboard sync |
  | `max_clipboard_bytes` | `1048576` | Largest inline clipboard payload |
  | `require_confirm` | `false` | Prompt before accepting incoming files |
  | `log_filter` | `"info"` | Default log level when `--verbose` is off |

- **`peers.toml`** — the paired-device registry. Written by `qdrop pair`
  (M2); `qdropd` reads it to decide who it will talk to.

A missing config file is not an error — `qdropd` runs on defaults.

---

## Logging

Logs go to **stderr**, colorized when stderr is a terminal and plain
otherwise (so `journald` / `launchd` capture stays readable). Precedence for
the log level:

1. `RUST_LOG` environment variable (full `tracing` filter syntax)
2. `--verbose` → `debug`
3. `log_filter` from `config.toml` → otherwise `info`

---

## What it does today

M1 (discovery + transport):

- Advertises `_qdrop._tcp.local.` over mDNS (TXT: device id, name, key
  fingerprint) and browses for other daemons on the LAN.
- Holds exactly **one** connection per peer. Arbitration: the daemon with the
  lexicographically lower device id dials, the other only accepts.
- `Hello` handshake exchanges protocol version + capabilities; the effective
  capability set (logged on both sides) is the intersection.
- `Ping`/`Pong` every 15s; a connection with no traffic for 45s is dropped.
- The dialer reconnects automatically with exponential backoff (1s→60s, full
  jitter). A clean peer disconnect reconnects almost immediately.

Still to come: encryption and pairing (M2, the transport is **plaintext TCP**
today), clipboard sync (M3), file transfer (M4+). See
[`../MILESTONES.md`](../MILESTONES.md).

The daemon writes a random device id to `~/.config/qdrop/device_id` on first
run; delete that file to get a new identity.
