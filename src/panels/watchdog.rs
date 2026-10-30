// SPDX-License-Identifier: GPL-3.0-only

//! Telling systemd the host is up and its UI alive (sd_notify), when run
//! by its unit: the beat comes from the UI's own loop, so a stuck UI misses
//! it and systemd restarts the host, the rest of the desktop untouched.

use std::os::unix::net::UnixDatagram;
use std::time::Duration;

/// Up and serving.
pub fn ready() {
    notify("READY=1");
}

/// Still alive.
pub fn beat() {
    notify("WATCHDOG=1");
}

/// How often to beat, when systemd watches this process: half its limit.
pub fn period() -> Option<Duration> {
    let usec = std::env::var("WATCHDOG_USEC").ok();
    let pid = std::env::var("WATCHDOG_PID").ok();
    period_of(usec.as_deref(), pid.as_deref(), std::process::id())
}

fn period_of(usec: Option<&str>, pid: Option<&str>, own: u32) -> Option<Duration> {
    let usec: u64 = usec?.trim().parse().ok().filter(|u| *u > 0)?;
    if let Some(pid) = pid
        && pid.trim().parse::<u32>().ok() != Some(own)
    {
        return None;
    }
    Some(Duration::from_micros(usec / 2))
}

fn notify(state: &str) {
    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let sent = UnixDatagram::unbound().and_then(|socket| {
        let bytes = path.as_encoded_bytes();
        let address = match bytes.strip_prefix(b"@") {
            Some(name) => {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(name)?
            }
            None => std::os::unix::net::SocketAddr::from_pathname(&path)?,
        };
        socket.send_to_addr(state.as_bytes(), &address)
    });
    if let Err(err) = sent {
        tracing::warn!("cannot tell systemd {state}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_at_half_the_limit_and_only_for_the_process_watched() {
        assert_eq!(period_of(Some("20000000"), None, 7), Some(Duration::from_secs(10)));
        assert_eq!(period_of(Some("20000000"), Some("7"), 7), Some(Duration::from_secs(10)));
        assert_eq!(period_of(Some("20000000"), Some("8"), 7), None);
        assert_eq!(period_of(None, None, 7), None);
        assert_eq!(period_of(Some("0"), None, 7), None);
        assert_eq!(period_of(Some("x"), None, 7), None);
    }

    #[test]
    fn the_notification_reaches_the_socket() {
        let dir = std::env::temp_dir().join(format!("central-notify-{}", std::process::id()));
        _ = std::fs::create_dir_all(&dir);
        let path = dir.join("notify");
        _ = std::fs::remove_file(&path);
        let socket = UnixDatagram::bind(&path).unwrap();
        // SAFETY: this test alone reads NOTIFY_SOCKET.
        unsafe { std::env::set_var("NOTIFY_SOCKET", &path) };
        ready();
        let mut buf = [0u8; 32];
        let n = socket.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        _ = std::fs::remove_dir_all(&dir);
    }
}
