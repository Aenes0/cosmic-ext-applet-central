// SPDX-License-Identifier: GPL-3.0-only

//! Running the system's programs, wpctl, pactl, cosmic-randr…: directly, or
//! from a Flatpak sandbox, which has none of them, on the host through
//! `flatpak-spawn --host`.

use std::sync::LazyLock;

static FLATPAK: LazyLock<bool> =
    LazyLock::new(|| std::path::Path::new("/.flatpak-info").exists());

pub fn in_flatpak() -> bool {
    *FLATPAK
}

/// A host program, with these environment variables. In a sandbox the
/// program ends with the applet, as it would outside.
pub fn command(program: &str, env: &[(&str, &str)]) -> tokio::process::Command {
    if in_flatpak() {
        let mut command = tokio::process::Command::new("flatpak-spawn");
        command.args(["--host", "--watch-bus"]);
        for (key, value) in env {
            command.arg(format!("--env={key}={value}"));
        }
        command.arg(program);
        command
    } else {
        let mut command = tokio::process::Command::new(program);
        command.envs(env.iter().copied());
        command
    }
}

/// Start a host program and leave it be.
pub fn spawn(program: &str, args: &[&str]) {
    let result = if in_flatpak() {
        std::process::Command::new("flatpak-spawn")
            .arg("--host")
            .arg(program)
            .args(args)
            .spawn()
    } else {
        std::process::Command::new(program).args(args).spawn()
    };
    match result {
        // Reaped by a thread, not left a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(err) => tracing::error!("cannot run {program}: {err}"),
    }
}

/// A file on the host: the sandbox has its own /tmp, where browsers leave
/// covers.
pub async fn read(path: &std::path::Path) -> Option<Vec<u8>> {
    if in_flatpak() {
        let output = command("cat", &[]).arg(path).output().await.ok()?;
        output.status.success().then_some(output.stdout)
    } else {
        tokio::fs::read(path).await.ok()
    }
}

/// When a file on the host last changed, in seconds since the epoch.
pub async fn modified(path: &std::path::Path) -> Option<u64> {
    if in_flatpak() {
        let output = command("stat", &[]).args(["-c", "%Y"]).arg(path).output().await.ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    } else {
        let modified = tokio::fs::metadata(path).await.ok()?.modified().ok()?;
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }
}
