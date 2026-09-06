# qdrop — Milestone Plan

Derived from [`qdropdesign.md`](qdropdesign.md). Each milestone is independently
demoable and ordered so the daily-driver cut (M1–M6) lands first, with hardening
and polish after.

---

## M0 — Project scaffold & CI

**Goal:** One repo, one binary, builds on both targets.

- Cargo workspace: `qdropd` (daemon) + `qdrop` (CLI) sharing a `qdrop-core` lib crate.
- `tokio` runtime, `clap` CLI skeleton with all subcommands stubbed
  (`pair`, `peers`, `send`, `open`, `clip`, `daemon`).
- Config loader for `~/.config/qdrop/config.toml` + `peers.toml` (serde, with defaults).
- Structured logging (`tracing`), `--verbose`, log to stderr / OS log.
- CI matrix: `{x86_64,aarch64}-apple-darwin` + `x86_64-unknown-linux-gnu`,
  `cargo clippy`, `cargo test`, `cargo fmt --check`.

**Done when:** `qdrop --help` and `qdropd --version` run on Mac + Omarchy from CI artifacts.

---

## M1 — Daemon skeleton: discovery + transport

**Goal:** Two daemons find each other and hold a connection.

- mDNS advertise + browse `_qdrop._tcp.local` via `mdns-sd`
  (device name, port, key fingerprint in TXT).
- Dial/accept arbitration: lower key fingerprint (or device-id) dials, other accepts;
  exactly one connection per peer pair.
- Length-prefixed msgpack framing (`rmp-serde`), `Hello` handshake with `caps` negotiation.
- `Ping`/`Pong` keepalive + idle timeout; reconnect with exponential backoff + jitter.
- **Plaintext TCP for this milestone** (TLS in M2), localhost + two-machine test.

**Done when:** kill Wi-Fi / sleep one box, it reconnects automatically within the
backoff window; `Hello` caps logged on both sides.

---

## M2 — Pairing & authenticated transport

**Goal:** Trust established once, enforced thereafter.

- `qdrop pair` (side A): advertise `_qdrop-pair._tcp` for 60s, print 6-digit PIN.
- `qdrop pair <name-or-ip>` (side B): SPAKE2 exchange keyed by PIN → shared secret;
  PIN never sent.
- Over the SPAKE2 channel: exchange long-term Ed25519 pubkeys, write peer entry to `peers.toml`.
- Steady state: `rustls` TLS 1.3, `rcgen` self-signed certs, mutual auth with
  **pinned** keys (custom cert verifier, no CA, no TOFU).
- Reject connections from unpinned keys; `qdrop peers` shows paired peers with
  online/offline + last seen.
- Unpair command (`qdrop pair --remove <name>`).

**Done when:** tampered PIN fails closed; connection with wrong/unknown key is refused;
re-pair after unpair works.

---

## M3 — Clipboard text sync (bidirectional)

**Goal:** Copy on one box, paste on the other.

- macOS: `NSPasteboard` poll `changeCount` ~250ms (via `arboard` + change detection).
- Omarchy: `wlr-data-control` watch via `wayland-client` (equivalent to `wl-paste --watch`).
- `Clipboard { seq, origin_id, entries }` frames, text MIME types only.
- Echo suppression: tag writes with `origin_id` + monotonic `seq`; on applying remote
  clipboard, record resulting local `changeCount` and ignore that echo.
- Per-direction enable flags; `qdrop clip --pause/--resume/--status`.
- `max_clipboard_bytes` guard.

**Done when:** rapid back-and-forth copy on both machines with no echo loop, no
oscillation; pause halts sync immediately.

---

## M4 — File send + blob streaming

**Goal:** `qdrop send <path>` lands on the peer.

- `BlobStart` / `BlobChunk` (~64 KB) / `BlobEnd { sha256 }` streaming; one blob in
  flight per connection (backpressure).
- `qdrop send <path>... [--to name]` — default to all paired peers, or the sole peer.
- Receive into `~/Downloads/qdrop/`; sanitize `name` (reject path traversal, absolute
  paths, `..`).
- SHA-256 verify on `BlobEnd`; atomic rename from temp file.
- Notifications: `UNUserNotification` (macOS helper) / `notify-send` (Linux).
- `require_confirm = true` config path → prompt before accepting.
- Resume/abort on connection drop mid-transfer (restart blob).

**Done when:** multi-file send, large file (>1 GB) send, and a corrupted-transfer case
all behave correctly; traversal payload `../../etc/x` is rejected.

---

## M5 — `qdrop open <url>`

**Goal:** Push a URL to the peer's browser.

- `OpenUrl { url }` frame; `open` (macOS) / `xdg-open` (Linux).
- URL scheme allowlist (`http`, `https`, maybe `mailto`) — no arbitrary command
  execution, no `file://` by default.
- `--to name` targeting; notification on receive.

**Done when:** `qdrop open https://…` focuses a browser tab on the peer;
`javascript:` / shell-ish payloads are rejected.

---

## M6 — Clipboard images + secret hygiene

**Goal:** Images sync; secrets never do.

- Image MIME support (PNG/TIFF ↔ PNG); route images through blob path when > 256 KB.
- **Skip** pasteboard items with `org.nspasteboard.ConcealedType` or `TransientType`;
  Linux equivalent heuristics (password-manager MIME hints).
- `sync_images` config toggle.
- Optional: suppress sync while peer's screen is locked (macOS
  `CGSessionCopyCurrentDictionary`, Linux idle/lock signal).
- Small file-list clipboard support (copy files in Finder → paste on peer) via blob path.

**Done when:** copying a 1Password field produces nothing on the peer; screenshot copy
appears on the peer; toggling `sync_images` takes effect without restart.

> **M1–M6 = the "use it every day" cut.**

---

## M7 — Status UI

**Goal:** At-a-glance state + quick pause.

- macOS menu bar (`tray-icon` + `muda`, or thin SwiftUI shell over a UDS): peer
  online/offline, pause/resume clipboard, last activity, open config.
- Omarchy waybar module: script emitting JSON (icon + tooltip with peer status),
  click → pause toggle.
- Daemon exposes a local status/control socket (UDS) the UI drives.
- Send triggers: macOS Services menu entry + Finder share extension; Hyprland keybind
  → `qdrop send $(fuzzel pick)`.

**Done when:** menu bar and waybar both reflect peer drop/reconnect within ~1s;
pause from UI is authoritative.

---

## M8 — Packaging & install

**Goal:** One-command install, autostart, survives reboot.

- macOS: `launchd` LaunchAgent plist; Homebrew tap formula.
- Omarchy: `systemd --user` unit; AUR-style `PKGBUILD` or install script.
- `qdrop daemon` as the service entrypoint; graceful shutdown, restart-on-crash.
- First-run UX: `qdrop pair` prompts if no peers configured.
- Man pages / `--help` polish, README with the 60-second setup.

**Done when:** fresh Mac + fresh Omarchy → install, pair, reboot both, sync still works
with no manual steps.

---

## M9 — Hardening & resilience

**Goal:** Trust it while you're not watching.

- Network transitions: Wi-Fi ↔ ethernet, IP change, VPN up/down, sleep/wake — all
  recover without restart.
- mDNS flakiness fallback: cache last-known peer IP:port, try direct dial before
  rediscovery.
- Fuzz the frame parser and blob reassembly; malformed-frame handling never panics
  the daemon.
- Resource caps: max in-flight memory, clipboard rate-limiting, oversized-frame rejection.
- Protocol version skew: `Hello.version` negotiation, refuse incompatible with a clear
  message.
- Observability: `qdrop peers --json`, connection/transfer metrics in logs.
- Threat-model doc + review of pinning, SPAKE2 usage, path sanitization, URL allowlist.

**Done when:** a week of daily use across sleep/roam cycles with zero manual
intervention; fuzz suite green in CI.

---

## Deferred (post-v1)

- Clipboard history (start last-value only).
- `… | qdrop send -` stdin support (cheap add — could fold into M4 if wanted).
- Internet relay / NAT traversal.
- More than a handful of devices; mobile.
