//! Local machine facts shared during the M10 authorization exchange.

use crate::proto::DeviceInfo;

/// Gather this machine's hostname, non-loopback MAC addresses, and OS.
pub fn local_device_info() -> DeviceInfo {
    DeviceInfo {
        hostname: hostname(),
        macs: local_macs(),
        os: std::env::consts::OS.to_string(),
    }
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Non-zero, non-loopback MAC addresses, lowercase `aa:bb:cc:dd:ee:ff`, sorted
/// and de-duplicated.
pub fn local_macs() -> Vec<String> {
    let mut out: Vec<String> = match mac_address::MacAddressIterator::new() {
        Ok(iter) => iter
            .filter(|m| m.bytes() != [0u8; 6])
            .map(|m| m.to_string().to_lowercase())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out.dedup();
    out
}

/// Whether `observed` still overlaps `recorded` — used as a soft "same
/// machine" check. An empty `recorded` (nothing was captured) always matches.
pub fn macs_still_match(recorded: &[String], observed: &[String]) -> bool {
    recorded.is_empty() || recorded.iter().any(|m| observed.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_is_populated() {
        let info = local_device_info();
        assert!(!info.hostname.is_empty());
        assert!(!info.os.is_empty());
        // MAC list may legitimately be empty in a locked-down sandbox.
        for m in &info.macs {
            assert_eq!(m.split(':').count(), 6, "{m:?}");
        }
    }

    #[test]
    fn soft_mac_match() {
        assert!(macs_still_match(&[], &["a".into()]));
        assert!(macs_still_match(
            &["aa:bb".into(), "cc:dd".into()],
            &["cc:dd".into()]
        ));
        assert!(!macs_still_match(&["aa:bb".into()], &[" zz:zz".into()]));
    }
}
