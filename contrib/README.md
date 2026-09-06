# contrib — status UI and send triggers (M7)

The daemon exposes a Unix control socket
(`$XDG_RUNTIME_DIR/qdropd.sock`, else `~/.config/qdrop/qdropd.sock`; override
with `QDROP_CONTROL_SOCK`). Everything here drives it through the `qdrop` CLI.

## macOS menu bar

A thin AppKit shell — `menubar/qdrop-menubar.swift`. It polls
`qdrop status --json` once a second and rebuilds its menu, so peer
drops/reconnects show within ~1s. Pause/Resume run `qdrop clip
--pause/--resume`, so the daemon stays authoritative.

```bash
swiftc -O -o /usr/local/bin/qdrop-menubar contrib/menubar/qdrop-menubar.swift
qdrop-menubar &        # or install as a LaunchAgent
```

## Omarchy / waybar

`waybar/qdrop.jsonc` is a `custom/qdrop` module. It runs
`qdrop status --waybar`, which prints one line of
`{"text","tooltip","class"}` JSON. Left-click pauses, middle-click resumes,
right-click opens the config folder. Style `.paused` / `.connected` / `.idle`
in your waybar CSS.

## Send triggers

**Hyprland keybind** (pick a file with fuzzel, send to the default peer):

```
bind = $mod SHIFT, S, exec, f=$(fd . ~ --type f | fuzzel --dmenu) && [ -n "$f" ] && qdrop send "$f"
```

**Hyprland keybind** (push the focused browser URL — needs `hyprctl` +
`wl-paste`; copy the URL first):

```
bind = $mod SHIFT, O, exec, qdrop open "$(wl-paste)"
```

**macOS Services**: create a Quick Action in Automator — "Run Shell Script",
input "files", `for f in "$@"; do /usr/local/bin/qdrop send "$f"; done` — and
it appears under Finder's right-click → Quick Actions and the Services menu.
