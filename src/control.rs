use std::{ffi::OsStr, os::unix::fs::MetadataExt, process::Command};

use anyhow::{Context, Result, bail};

#[derive(Clone, Copy)]
pub enum VisibilityCommand {
    Hide,
    Show,
    Toggle,
}

impl VisibilityCommand {
    pub fn from_arg(arg: &OsStr) -> Option<Self> {
        match arg.to_str()? {
            "--hide" => Some(Self::Hide),
            "--show" => Some(Self::Show),
            "--toggle" => Some(Self::Toggle),
            _ => None,
        }
    }

    pub fn send(self) -> Result<()> {
        let signal = match self {
            Self::Hide => "USR2",
            Self::Show => "HUP",
            Self::Toggle => "USR1",
        };
        let uid = std::fs::metadata("/proc/self")
            .context("failed to determine the user")?
            .uid()
            .to_string();
        // Exclude this short-lived nibari command (pkill's ancestor) and other users.
        let output = Command::new("pkill")
            .args(["-A", "-u", &uid, "--signal", signal, "-x", "nibari"])
            .output()
            .context("failed to start pkill (procps is required)")?;
        if output.status.success() {
            return Ok(());
        }
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            bail!("nibari bar is not running");
        }
        bail!(
            "failed to send the command to the bar ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}
