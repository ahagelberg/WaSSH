//! Installing and managing WSL environments.
//!
//! A distribution install, enabling WSL itself and unregistering a distribution all take
//! time, so they run on their own thread and the dialog polls the channel each frame; the
//! thread wakes the window once it has the answer.

use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};

use wassh_term::wsl;

/// The WSL launcher.
const WSL: &str = "wsl.exe";
/// Windows PowerShell, which starts the installer elevated.
const POWERSHELL: &str = "powershell.exe";
/// Enables WSL itself, with the elevation it needs and without a distribution.
///
/// The app never runs elevated, and `wsl --install` cannot enable its optional components
/// without administrator rights, so Windows asks for approval and the user picks a
/// distribution in the dialog afterwards.
const INSTALL_WSL_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'; \
$installer = Start-Process -FilePath wsl.exe -ArgumentList '--install','--no-distribution' \
  -Verb RunAs -PassThru -Wait; \
exit $installer.ExitCode";

/// A running WSL command, and the answer once it finishes.
pub struct WslTask {
    /// What the command printed, or why it could not finish.
    pub output: Receiver<Result<String, String>>,
}

/// Install `distro` in the background.
pub fn install(distro: &str) -> WslTask {
    let distro = distro.to_owned();
    spawn(move || {
        let output = run(&[
            "--install",
            "--distribution",
            &distro,
            // Its first run stays in WaSSH's tab, where the Linux user is set up.
            "--no-launch",
        ])?;
        // `wsl.exe` exits zero even when it did nothing — enabling a Windows component reports
        // "successful" but only takes effect after a reboot — so ask what is installed now.
        confirmed(&distro, &wsl::distros(), output)
    })
}

/// Whether the install really registered the distribution, whatever the exit code said.
fn confirmed(distro: &str, installed: &[String], output: String) -> Result<String, String> {
    if installed.iter().any(|name| name == distro) {
        return Ok(output);
    }
    // Enabling a Windows component reports success but only takes effect after a restart.
    if output.to_lowercase().contains("reboot") {
        return Err(format!(
            "Windows must be restarted before {distro} can be installed."
        ));
    }
    if output.trim().is_empty() {
        Err(format!("WSL did not report {distro} as installed."))
    } else {
        Err(format!("WSL did not install {distro}: {output}"))
    }
}

/// Enable WSL itself, asking Windows for the elevation that needs.
pub fn install_wsl() -> WslTask {
    spawn(run_wsl_installer)
}

/// Unregister `distro`, deleting the distribution and everything in it.
pub fn remove(distro: &str) -> WslTask {
    let distro = distro.to_owned();
    spawn(move || {
        let output = run(&["--unregister", &distro])?;
        // As with installs, trust the listing rather than the exit code.
        if wsl::distros().iter().any(|name| name == &distro) {
            Err(if output.trim().is_empty() {
                format!("WSL did not remove {distro}.")
            } else {
                output
            })
        } else {
            Ok(output)
        }
    })
}

/// Make `distro` the one WSL starts when no distribution is named.
pub fn set_default(distro: &str) -> WslTask {
    let distro = distro.to_owned();
    spawn(move || run(&["--set-default", &distro]))
}

fn spawn(job: impl FnOnce() -> Result<String, String> + Send + 'static) -> WslTask {
    let (sender, output) = channel();
    let unused = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("wassh-wsl-task".to_owned())
        .spawn(move || {
            let _ = sender.send(job());
            wassh_core::wake::notify();
        });
    if let Err(error) = spawned {
        let _ = unused.send(Err(format!("starting the WSL command: {error}")));
        wassh_core::wake::notify();
    }
    WslTask { output }
}

fn run(args: &[&str]) -> Result<String, String> {
    let output = Command::new(WSL)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running {WSL}: {error}"))?;
    let mut text = wsl::decode_console_output(&output.stdout);
    text.push_str(&wsl::decode_console_output(&output.stderr));
    verdict(output.status.success(), text.trim(), WSL)
}

fn run_wsl_installer() -> Result<String, String> {
    let output = Command::new(POWERSHELL)
        .args(["-NoProfile", "-NonInteractive", "-Command", INSTALL_WSL_SCRIPT])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running {POWERSHELL}: {error}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    verdict(output.status.success(), text.trim(), "The WSL installer")
}

fn verdict(ok: bool, text: &str, what: &str) -> Result<String, String> {
    if ok {
        Ok(text.to_owned())
    } else if text.is_empty() {
        Err(format!("{what} failed."))
    } else {
        Err(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_zero_only_counts_when_wsl_lists_the_distribution() {
        let installed = vec!["Ubuntu".to_owned()];
        assert_eq!(
            confirmed("Ubuntu", &installed, "done".to_owned()),
            Ok("done".to_owned())
        );
        assert_eq!(
            confirmed("Debian", &installed, "Access is denied.".to_owned()),
            Err("WSL did not install Debian: Access is denied.".to_owned())
        );
        assert_eq!(
            confirmed("Debian", &installed, String::new()),
            Err("WSL did not report Debian as installed.".to_owned())
        );
        assert_eq!(
            confirmed(
                "Debian",
                &installed,
                "The requested operation is successful. Changes will not be effective until the \
                 system is rebooted."
                    .to_owned()
            ),
            Err("Windows must be restarted before Debian can be installed.".to_owned())
        );
    }
}
