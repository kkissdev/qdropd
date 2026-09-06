# qdrop — Milestone Plan

Derived from [`qdropdesign.md`](qdropdesign.md). Each milestone is independently
demoable and ordered so the daily-driver cut (M1–M6) lands first, with hardening
and polish after. M0–M9 are implemented; M10–M11 are planned.

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

## M10 — Pre-authorized senders (`qdrop auth`)

**Goal:** Bless a machine once so it can push to you forever without a prompt.

Pairing (M2) establishes *mutual trust to connect*. This adds a second, opt-in
layer on top: a per-peer "this device may drop things on me unattended" flag,
so `require_confirm` (M4) can stay on for the world but off for the laptop you
own.

- `qdrop auth <name-or-id>` — run it while already paired **and connected** to
  the target. The daemon sends an `AuthRequest` over the live (pinned-key TLS)
  connection; the peer replies `AuthReply { hostname, macs, os }` with the MAC
  addresses of its non-loopback interfaces. We record `authorized = true`,
  `hostname`, and `macs` on that peer's entry in `peers.toml`.
- New frames `AuthRequest` / `AuthReply`; `caps.auth` negotiated in `Hello` so
  an older peer degrades gracefully (command errors with "peer too old").
- Enforcement: an authorized peer's incoming files / URLs bypass the
  confirmation gate — they land straight in `~/Downloads/qdrop/` even when
  `require_confirm = true`. Unlisted peers are unaffected (still prompted /
  routed to `pending/`). `require_confirm = "strict"` opts back in to prompting
  everyone, authorized or not.
- The **pinned Ed25519 key stays the security boundary.** The stored MAC is a
  human-readable "which physical machine" label and a soft check: on a later
  connect, a MAC that no longer matches any recorded value logs a warning (and
  a desktop notification) but does **not** revoke authorization — MACs are
  randomized/rotated and not trustworthy on their own.
- `qdrop auth --remove <name>` revokes (back to prompting). `qdrop auth`
  (no arg) or `qdrop peers` lists which peers are authorized, with the MAC and
  hostname last seen.
- Authorization is **one-directional**: `qdrop auth beta` lets *beta* push to
  *this* machine unattended. `qdrop auth --mutual beta` asks beta to authorize
  us back in the same exchange.

**Done when:** with `require_confirm = true`, a file from an authorized peer
lands directly in `~/Downloads/qdrop/` while a file from an unlisted peer still
lands in `pending/`; `qdrop auth --remove` restores the prompt; reconnecting
the authorized peer from a different network (new MAC) still works and only
logs a notice.

---

## M11 — Beyond the LAN (static endpoints + relay)

**Goal:** Reach a paired device that isn't on the same network — a laptop on
hotel Wi-Fi, a box at another site — without weakening anything.

The M2 handshake is already end-to-end (pinned Ed25519 keys, mutual TLS 1.3),
so **every path added here is untrusted plumbing**: a router, a VPN, or a relay
can move bytes but cannot read them or join a session.

### Static endpoints (no infrastructure)

- `peers.toml` entry gains an optional `endpoint = "host:port"` (also
  `qdrop endpoint <name> <host:port>` / `qdrop endpoint --clear <name>`).
- Dial order becomes: cached last-known addr → mDNS-discovered addrs →
  static endpoint. The static endpoint covers a manual router port-forward, a
  DNS name, or a mesh VPN address (Tailscale / WireGuard / Nebula).
- `qdrop pair <host:port>` works over the internet too (SPAKE2 doesn't care
  about the transport), so first-contact across networks needs no LAN moment.

### Relay (works behind two NATs, no port-forward)

- Optional `qdrop-relay` server (a tiny binary; run one yourself, or point at a
  shared one). Both peers make an **outbound** TLS/WebSocket connection to it on
  443, so NAT and firewalls are non-issues.
- The relay pairs two streams by a **rendezvous token** = a hash over the two
  peers' pinned public keys (each side can compute it; the relay learns nothing
  from it). It then splices the streams and forwards ciphertext blindly. It has
  no pinned key, so it cannot impersonate either side or MITM the qdrop
  handshake that runs on top.
- `config.toml` gains `relay = "wss://relay.example.org"`. The daemon keeps one
  idle control connection to the relay with keepalives; when a direct path to a
  peer is unavailable it opens a relayed session on demand.
- **Direct is always preferred.** A relayed session periodically re-probes for
  a direct path (LAN rediscovery, static endpoint) and migrates to it
  transparently, then drops the relay leg — like Tailscale's DERP→direct
  upgrade. `qdrop status` / `qdrop peers` show `direct` vs `via relay`.
- *Stretch:* use the relay as a STUN-style coordinator to exchange candidate
  addresses and UDP hole-punch a direct P2P link before falling back to
  forwarding.

### Notes

- Discovery over the relay: a peer registers "online" under its device id;
  another peer asks "is X reachable?" and the relay connects them if both are
  present. No presence is shared beyond peers you've paired with.
- Threat model addendum: a relay operator sees **metadata** — which device ids
  talk, when, and roughly how much — but never content. Running your own relay
  (or using a VPN endpoint instead) removes even that.
- Everything is opt-in: with no `endpoint` and no `relay` configured, qdrop
  stays exactly as LAN-only as it is today.

**Done when:** two machines on different networks, both behind NAT with no
port-forwarding, pair with `qdrop pair` and sync clipboard + files through the
relay; when they later join the same LAN they switch to a direct connection
with no user action and `qdrop status` flips from `via relay` to `direct`;
stopping the relay only affects peers that have no direct path.

---

## Deferred (post-v1)

- Clipboard history (start last-value only).
- `… | qdrop send -` stdin support (cheap add — could fold into M4 if wanted).
- More than a handful of devices; mobile.
