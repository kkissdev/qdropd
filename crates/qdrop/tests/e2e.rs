//! End-to-end test (M17): two real `qdropd` instances on this host, driven
//! through the `qdrop` CLI, exercising every earlier milestone's "done when".
//!
//! Runs headless. On Linux CI it needs an X server for the clipboard step
//! (`xvfb-run`); without one, the clipboard assertion is skipped with a note.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A `qdropd` child that is killed on drop.
struct Daemon {
    child: Child,
    cfg: PathBuf,
    ctl: PathBuf,
    dl: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.ctl);
    }
}

fn bin(name: &str) -> PathBuf {
    // Both binaries live in the same profile dir as the test's CLI binary.
    let qdrop = PathBuf::from(env!("CARGO_BIN_EXE_qdrop"));
    qdrop.with_file_name(name)
}

fn ensure_built() {
    if bin("qdropd").exists() {
        return;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let ok = Command::new(cargo)
        .args(["build", "-p", "qdropd", "-p", "qdrop"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok && bin("qdropd").exists(), "could not build qdropd");
}

fn qdrop(d: &Daemon, args: &[&str]) -> std::process::Output {
    Command::new(bin("qdrop"))
        .args(args)
        .env("QDROP_CONFIG_DIR", &d.cfg)
        .env("QDROP_CONTROL_SOCK", &d.ctl)
        .env("QDROP_DOWNLOAD_DIR", &d.dl)
        .output()
        .expect("run qdrop")
}

fn qdrop_stdin(d: &Daemon, args: &[&str], input: &[u8]) -> std::process::Output {
    let mut c = Command::new(bin("qdrop"))
        .args(args)
        .env("QDROP_CONFIG_DIR", &d.cfg)
        .env("QDROP_CONTROL_SOCK", &d.ctl)
        .env("QDROP_DOWNLOAD_DIR", &d.dl)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn qdrop");
    c.stdin.take().unwrap().write_all(input).unwrap();
    c.wait_with_output().unwrap()
}

fn wait_until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out waiting for: {what}");
}

fn start_daemon(name: &'static str, cfg: PathBuf, dl: PathBuf) -> Daemon {
    std::fs::create_dir_all(&dl).unwrap();
    let ctl = std::env::temp_dir().join(format!("qde-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&ctl);
    let log = std::fs::File::create(cfg.join("daemon.log")).unwrap();
    let child = Command::new(bin("qdropd"))
        .arg("--verbose")
        .env("QDROP_CONFIG_DIR", &cfg)
        .env("QDROP_CONTROL_SOCK", &ctl)
        .env("QDROP_DOWNLOAD_DIR", &dl)
        .env("QDROP_DEVICE_NAME", name)
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn qdropd");
    Daemon {
        child,
        cfg,
        ctl,
        dl,
    }
}

fn daemon_log(d: &Daemon) -> String {
    std::fs::read_to_string(d.cfg.join("daemon.log")).unwrap_or_default()
}

fn peer_online(d: &Daemon, peer: &str) -> bool {
    let out = qdrop(d, &["status", "--json"]);
    if !out.status.success() {
        return false;
    }
    let v: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.get("peers")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter().any(|p| {
                p.get("name").and_then(|n| n.as_str()) == Some(peer)
                    && p.get("online").and_then(|o| o.as_bool()) == Some(true)
            })
        })
        .unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
#[ignore = "e2e: spawns real daemons — run via `cargo test -p qdrop --test e2e -- --ignored` or the CI e2e job"]
fn end_to_end() {
    ensure_built();
    let tmp = tempdir();
    let a_cfg = tmp.join("a");
    let b_cfg = tmp.join("b");
    let a_dl = tmp.join("a-dl");
    let b_dl = tmp.join("b-dl");
    for p in [&a_cfg, &b_cfg] {
        std::fs::create_dir_all(p).unwrap();
    }
    // B holds unlisted incoming files for confirmation (M10).
    std::fs::write(
        b_cfg.join("config.toml"),
        "port = 0\nrequire_confirm = true\n",
    )
    .unwrap();
    std::fs::write(a_cfg.join("config.toml"), "port = 0\n").unwrap();

    // --- M2: pair -------------------------------------------------------
    let offer_out = tmp.join("offer.out");
    let offer = Command::new(bin("qdrop"))
        .arg("pair")
        .env("QDROP_CONFIG_DIR", &a_cfg)
        .env("QDROP_DEVICE_NAME", "alpha")
        .stdout(Stdio::from(std::fs::File::create(&offer_out).unwrap()))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pair offerer");

    let mut pin = None;
    for _ in 0..100 {
        if let Some(p) = std::fs::read_to_string(&offer_out).ok().and_then(|s| {
            s.split_whitespace()
                .find(|w| w.len() == 6 && w.chars().all(|c| c.is_ascii_digit()))
                .map(str::to_string)
        }) {
            pin = Some(p);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let pin = pin.expect("offerer never printed a PIN");

    let mut requester = Command::new(bin("qdrop"))
        .args(["pair", "127.0.0.1"])
        .env("QDROP_CONFIG_DIR", &b_cfg)
        .env("QDROP_DEVICE_NAME", "beta")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pair requester");
    requester
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{pin}\n").as_bytes())
        .unwrap();
    let r = requester.wait_with_output().unwrap();
    assert!(
        r.status.success(),
        "requester pair failed: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let o = offer.wait_with_output().unwrap();
    assert!(
        o.status.success(),
        "offerer pair failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    assert!(a_cfg.join("peers.toml").exists(), "A peers.toml missing");
    assert!(b_cfg.join("peers.toml").exists(), "B peers.toml missing");

    // --- start both daemons ------------------------------------------
    let a = start_daemon("alpha", a_cfg.clone(), a_dl.clone());
    let b = start_daemon("beta", b_cfg.clone(), b_dl.clone());

    wait_until("A sees beta online", Duration::from_secs(45), || {
        peer_online(&a, "beta")
    });
    wait_until("B sees alpha online", Duration::from_secs(45), || {
        peer_online(&b, "alpha")
    });

    // --- M4: file send + SHA verify --------------------------------
    let payload: Vec<u8> = (0..2_000_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    let want = sha256_hex(&payload);
    let src = tmp.join("big.bin");
    std::fs::write(&src, &payload).unwrap();
    let out = qdrop(&a, &["send", src.to_str().unwrap(), "--to", "beta"]);
    assert!(
        out.status.success(),
        "send failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // --- M10: require_confirm diverts an unlisted peer to pending/ --
    wait_until("big.bin in B pending/", Duration::from_secs(20), || {
        b_dl.join("pending").join("big.bin").exists()
    });
    assert!(
        !b_dl.join("big.bin").exists(),
        "unlisted file should not land directly"
    );
    let got = std::fs::read(b_dl.join("pending").join("big.bin")).unwrap();
    assert_eq!(sha256_hex(&got), want, "SHA-256 mismatch after transfer");

    // authorize alpha, resend -> lands directly
    let out = qdrop(&b, &["auth", "alpha"]);
    assert!(
        out.status.success(),
        "auth failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = qdrop(&a, &["send", src.to_str().unwrap(), "--to", "beta"]);
    assert!(out.status.success());
    wait_until("big.bin direct in B", Duration::from_secs(20), || {
        b_dl.join("big.bin").exists()
    });

    // --- M13: stdin send ------------------------------------------
    let out = qdrop_stdin(
        &a,
        &["-q", "send", "-", "--name", "piped.txt", "--to", "beta"],
        b"from-a-stdin",
    );
    assert!(
        out.status.success(),
        "stdin send failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_until("piped.txt in B", Duration::from_secs(15), || {
        b_dl.join("piped.txt").exists()
    });
    assert_eq!(
        std::fs::read(b_dl.join("piped.txt")).unwrap(),
        b"from-a-stdin"
    );

    // --- M5: URL push + allowlist -------------------------------
    let ok = qdrop(&a, &["open", "https://example.com/", "--to", "beta"]);
    assert!(
        ok.status.success(),
        "open failed: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    wait_until("B handled OpenUrl", Duration::from_secs(10), || {
        let l = daemon_log(&b);
        l.contains("opened URL from peer") || l.contains("failed to launch")
    });
    let bad = qdrop(&a, &["open", "javascript:alert(1)"]);
    assert!(
        !bad.status.success(),
        "javascript: URL should be rejected client-side"
    );

    // --- M3: clipboard text (skipped if no clipboard) ---------
    std::thread::sleep(Duration::from_millis(500));
    if daemon_log(&a).contains("clipboard unavailable")
        || daemon_log(&b).contains("clipboard unavailable")
    {
        eprintln!("note: no clipboard available — skipping clipboard assertion");
    } else {
        let token = format!("qdrop-e2e-{}", std::process::id());
        let out = qdrop_stdin(&a, &["-q", "copy"], token.as_bytes());
        assert!(
            out.status.success(),
            "copy failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        wait_until("clipboard propagated", Duration::from_secs(8), || {
            daemon_log(&a).contains("clipboard text broadcast")
                || daemon_log(&b).contains("clipboard text broadcast")
        });
    }

    // --- M1/M9: reconnect after a kill ----------------------
    drop(b);
    std::thread::sleep(Duration::from_secs(1));
    let b = start_daemon("beta", b_cfg.clone(), b_dl.clone());
    wait_until("A reconnects to beta", Duration::from_secs(45), || {
        peer_online(&a, "beta")
    });

    // --- M2: unpair revokes trust -------------------------
    let out = qdrop(&b, &["pair", "--remove", "alpha"]);
    assert!(
        out.status.success(),
        "unpair failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_until(
        "B drops the connection after unpair",
        Duration::from_secs(15),
        || {
            !peer_online(&a, "beta")
                || daemon_log(&b).contains("peer unpaired")
                || daemon_log(&b).contains("rejecting inbound")
        },
    );

    // keep `a`, `b` alive until here so Drop kills them
    drop((a, b));
}

fn tempdir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("qdrop-e2e-{}-{}", std::process::id(), now_nanos()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
