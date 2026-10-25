//! One-shot remote commands on an existing connection.
//!
//! Each call opens its own session channel, runs the command and lets the channel close, so a
//! probe for system statistics never disturbs the interactive shell.

use std::sync::Arc;

use russh::ChannelMsg;
use russh::client::Handle;

use crate::session::{ClientHandler, ExecOutput};

/// Run `command` through the remote shell and collect its output.
pub(crate) async fn run(
    handle: Arc<Handle<ClientHandler>>,
    command: String,
    input: Option<String>,
) -> Result<ExecOutput, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|error| format!("opening a command channel: {error}"))?;
    channel
        .exec(true, command.as_bytes().to_vec())
        .await
        .map_err(|error| format!("running `{command}`: {error}"))?;

    // A `sudo -S` preamble reads its password from stdin.
    if let Some(input) = input {
        channel
            .data(input.as_bytes())
            .await
            .map_err(|error| format!("writing to `{command}`: {error}"))?;
        let _ = channel.eof().await;
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status: status } => exit_status = Some(status),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(ExecOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_keeps_its_channels_apart() {
        let output = ExecOutput {
            stdout: "up 3 days\n".to_owned(),
            stderr: String::new(),
            exit_status: Some(0),
        };
        assert_eq!(output.exit_status, Some(0));
        assert!(output.stderr.is_empty());
    }
}
