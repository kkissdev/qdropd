//! Best-effort desktop notifications. macOS via `osascript`, Linux via
//! `notify-send`. Failures are logged at debug and otherwise ignored.

use std::process::Stdio;

/// Post a notification. Non-blocking-ish: spawns the helper and forgets it.
pub fn notify(summary: &str, body: &str) {
    let result = if cfg!(target_os = "macos") {
        let script = format!(
            "display notification {} with title {}",
            applescript_quote(body),
            applescript_quote(summary),
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        std::process::Command::new("notify-send")
            .args(["--app-name=qdrop", summary, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    if let Err(e) = result {
        tracing::debug!("notification helper unavailable: {e}");
    }
}

fn applescript_quote(s: &str) -> String {
    // AppleScript string literal: wrap in quotes, backslash-escape " and \.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
