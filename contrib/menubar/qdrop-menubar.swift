// qdrop menu-bar app for macOS — a thin AppKit shell over the daemon's
// control socket. It shells out to the `qdrop` CLI (which talks to the socket)
// so it has no dependency on the daemon internals.
//
// Build:  swiftc -O -o qdrop-menubar qdrop-menubar.swift
// Run:    ./qdrop-menubar   (or drop it in a LaunchAgent — see docs/qdropd.md)
//
// It polls `qdrop status --json` once a second and rebuilds the menu, so peer
// drops / reconnects show up within ~1s. Pause / Resume run `qdrop clip
// --pause/--resume`, so the daemon stays authoritative.

import AppKit

final class Controller: NSObject, NSApplicationDelegate {
    let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    var timer: Timer?

    func applicationDidFinishLaunching(_ note: Notification) {
        item.button?.title = "qdrop"
        rebuild()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { [weak self] _ in
            self?.rebuild()
        }
    }

    // Run the qdrop CLI and return stdout.
    func qdrop(_ args: [String]) -> Data? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        p.arguments = ["qdrop"] + args
        let out = Pipe()
        p.standardOutput = out
        p.standardError = Pipe()
        do { try p.run() } catch { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return p.terminationStatus == 0 ? data : nil
    }

    func rebuild() {
        let menu = NSMenu()
        var online = 0
        var paused = false
        var running = false

        if let data = qdrop(["status", "--json"]),
           let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            running = true
            paused = json["clipboard_paused"] as? Bool ?? false
            let version = json["version"] as? String ?? "?"
            menu.addItem(disabled("qdrop \(version)"))
            let peers = json["peers"] as? [[String: Any]] ?? []
            if peers.isEmpty {
                menu.addItem(disabled("No paired peers"))
            }
            for peer in peers {
                let name = peer["name"] as? String ?? "?"
                let up = peer["online"] as? Bool ?? false
                if up { online += 1 }
                menu.addItem(disabled("  \(up ? "●" : "○") \(name)"))
            }
        } else {
            menu.addItem(disabled("daemon not running"))
        }

        menu.addItem(.separator())
        if running {
            if paused {
                menu.addItem(action("Resume clipboard sync", #selector(resume)))
            } else {
                menu.addItem(action("Pause clipboard sync", #selector(pause)))
            }
        }
        menu.addItem(action("Open config folder", #selector(openConfig)))
        menu.addItem(.separator())
        menu.addItem(action("Quit qdrop-menubar", #selector(quit)))

        item.menu = menu
        item.button?.title = paused ? "⏸ qdrop" : (running ? "qdrop \(online)" : "qdrop ⚠")
    }

    func disabled(_ title: String) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        i.isEnabled = false
        return i
    }
    func action(_ title: String, _ sel: Selector) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: sel, keyEquivalent: "")
        i.target = self
        return i
    }

    @objc func pause() { _ = qdrop(["clip", "--pause"]); rebuild() }
    @objc func resume() { _ = qdrop(["clip", "--resume"]); rebuild() }
    @objc func openConfig() {
        let dir = ("~/.config/qdrop" as NSString).expandingTildeInPath
        NSWorkspace.shared.open(URL(fileURLWithPath: dir))
    }
    @objc func quit() { NSApp.terminate(nil) }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory) // menu-bar only, no Dock icon
let controller = Controller()
app.delegate = controller
app.run()
