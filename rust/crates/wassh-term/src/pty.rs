//! Local pseudo-terminal transport.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::grid::TerminalSize;

/// A running local shell attached to a PTY.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
    size: TerminalSize,
}

impl PtySession {
    /// Spawn `shell` with a PTY of the given size.
    pub fn spawn(shell: &str, args: &[String], size: TerminalSize, term_type: &str) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(size))
            .context("opening a pseudo-terminal")?;

        let mut command = CommandBuilder::new(shell);
        for arg in args {
            command.arg(arg);
        }
        command.env("TERM", term_type);
        command.env("COLORTERM", "truecolor");
        if let Some(home) = std::env::var_os("HOME") {
            command.cwd(home);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("spawning {shell}"))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("cloning the PTY reader")?;
        let writer = pair.master.take_writer().context("taking the PTY writer")?;

        let (tx, output) = channel();
        thread::Builder::new()
            .name("wassh-pty-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buffer[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .context("starting the PTY reader thread")?;

        Ok(Self {
            master: pair.master,
            writer,
            child,
            output,
            size,
        })
    }

    /// Default interactive shell for this platform.
    pub fn default_shell() -> (String, Vec<String>) {
        #[cfg(target_os = "windows")]
        {
            let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".to_owned());
            (shell, Vec::new())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
            (shell, vec!["-l".to_owned()])
        }
    }

    /// Bytes produced since the last call; never blocks.
    pub fn read_pending(&self) -> Vec<Vec<u8>> {
        self.output.try_iter().collect()
    }

    /// Whether the child process has finished (and been reaped).
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("writing to the PTY")?;
        self.writer.flush().context("flushing the PTY")?;
        Ok(())
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        if size == self.size {
            return Ok(());
        }
        self.size = size;
        self.master
            .resize(pty_size(size))
            .context("resizing the PTY")?;
        Ok(())
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Terminate the child process.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_size_tracks_terminal_size() {
        let size = pty_size(TerminalSize::new(120, 40));
        assert_eq!(size.cols, 120);
        assert_eq!(size.rows, 40);
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_command_and_reads_output() {
        let mut pty = PtySession::spawn(
            "/bin/sh",
            &["-c".to_owned(), "printf 'hello from pty'".to_owned()],
            TerminalSize::new(40, 10),
            "xterm-256color",
        )
        .expect("spawn");

        let mut collected = String::new();
        for _ in 0..100 {
            for chunk in pty.read_pending() {
                collected.push_str(&String::from_utf8_lossy(&chunk));
            }
            if collected.contains("hello from pty") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(collected.contains("hello from pty"), "got: {collected:?}");
        pty.kill();
    }
}
