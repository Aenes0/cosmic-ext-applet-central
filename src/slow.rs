// SPDX-License-Identifier: GPL-3.0-only

//! What can take long when a panel opens (the GPU's library, /proc, DDC, a
//! Wi-Fi scan, the clipboard's file), each with how long it may: off the
//! UI's thread, and given up past that, so a stuck one never holds a panel.

use std::future::Future;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slow {
    /// A system reading: /proc, sensors, NVML.
    Sample,
    /// Monitors' brightness over DDC: a few tries, each a while.
    Ddc,
    /// Wi-Fi networks, a scan included.
    Wifi,
    /// The clipboard's history, from its file.
    Clipboard,
    /// The display's brightness, from COSMIC's settings daemon.
    Brightness,
}

impl Slow {
    #[cfg(test)]
    pub const ALL: [Slow; 5] = [Slow::Sample, Slow::Ddc, Slow::Wifi, Slow::Clipboard, Slow::Brightness];

    pub const fn limit(self) -> Duration {
        match self {
            Slow::Sample => Duration::from_secs(5),
            Slow::Ddc => Duration::from_secs(8),
            // The scan waits three seconds before listing.
            Slow::Wifi => Duration::from_secs(10),
            Slow::Clipboard => Duration::from_secs(2),
            Slow::Brightness => Duration::from_secs(3),
        }
    }
}

/// The future's output, or none if it took longer than allowed.
pub async fn bounded<F: Future>(slow: Slow, future: F) -> Option<F::Output> {
    let output = within(slow.limit(), future).await;
    if output.is_none() {
        tracing::warn!("{slow:?} took over {:?}: given up", slow.limit());
    }
    output
}

/// The work done on a thread of its own, which yields to anything else
/// that wants the CPU (the UI, the compositor); none if it took longer than
/// allowed (it is left to finish alone) or failed.
pub async fn blocking<T: Send + 'static>(slow: Slow, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (done, result) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new().name("central-slow".into()).spawn(move || {
        idle_priority();
        _ = done.send(work());
    });
    if let Err(err) = spawned {
        tracing::warn!("no thread for {slow:?}: {err}");
        return None;
    }
    bounded(slow, result).await?.ok()
}

/// The calling thread runs only when nothing else would (SCHED_IDLE); if
/// that's refused, at the lowest niceness. Never undone: the thread is
/// this work's alone.
fn idle_priority() {
    let param = libc::sched_param { sched_priority: 0 };
    // SAFETY: 0 is the calling thread; the parameter is valid for SCHED_IDLE.
    if unsafe { libc::sched_setscheduler(0, libc::SCHED_IDLE, &param) } != 0 {
        // SAFETY: a thread's own niceness, by its id.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 19) };
    }
}

async fn within<F: Future>(limit: Duration, future: F) -> Option<F::Output> {
    tokio::time::timeout(limit, future).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_limit_lets_work_be_done_and_none_holds_long() {
        for slow in Slow::ALL {
            assert!(slow.limit() >= Duration::from_secs(1), "{slow:?}");
            assert!(slow.limit() <= Duration::from_secs(10), "{slow:?}");
        }
        // The scan alone waits three seconds.
        assert!(Slow::Wifi.limit() > Duration::from_secs(3));
    }

    #[tokio::test]
    async fn what_never_ends_is_given_up() {
        assert_eq!(within(Duration::from_millis(20), std::future::pending::<()>()).await, None);
        assert_eq!(within(Duration::from_millis(20), async { 7 }).await, Some(7));
    }

    #[tokio::test]
    async fn blocking_work_comes_back() {
        assert_eq!(blocking(Slow::Clipboard, || 3).await, Some(3));
    }

    #[tokio::test]
    async fn blocking_work_runs_at_idle_priority_and_the_caller_does_not() {
        let policy = || unsafe { libc::sched_getscheduler(0) };
        assert_eq!(blocking(Slow::Clipboard, policy).await, Some(libc::SCHED_IDLE));
        assert_ne!(policy(), libc::SCHED_IDLE);
    }
}
