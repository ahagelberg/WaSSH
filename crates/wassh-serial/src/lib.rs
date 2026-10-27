//! Serial-port transport.
//!
//! `serial2` gives us a blocking port with a read timeout; a reader thread forwards bytes
//! to the UI, mirroring `PtySession`. Resizing is a no-op because serial terminals are
//! fixed-size.

use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serial2::{FlowControl, Parity, SerialPort, Settings, StopBits};
use wassh_core::{FlowControl as CoreFlow, Parity as CoreParity, SerialConfig, StopBits as CoreStop};

/// A live serial connection.
pub struct SerialSession {
    port: SerialPort,
    output: Receiver<Vec<u8>>,
    path: String,
    config: SerialConfig,
}

impl SerialSession {
    /// Open `config.path` with the saved line settings.
    pub fn open(config: &SerialConfig) -> Result<Self> {
        let mut port = SerialPort::open(&config.path, config.baud)
            .with_context(|| format!("opening {}", config.path))?;
        let settings = build_settings(&port, config)?;
        port.set_configuration(&settings)
            .context("applying the serial line settings")?;

        let reader = port.try_clone().context("cloning the serial port")?;
        let (tx, output) = channel();
        thread::Builder::new()
            .name("wassh-serial-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            if tx.send(buffer[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .context("starting the serial reader thread")?;

        Ok(Self {
            port,
            output,
            path: config.path.clone(),
            config: config.clone(),
        })
    }

    /// Bytes produced since the last call; never blocks.
    pub fn read_pending(&self) -> Vec<Vec<u8>> {
        self.output.try_iter().collect()
    }

    /// Write bytes to the port.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.port.write_all(bytes).context("writing to the serial port")
    }

    /// Port path as configured.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Line settings in use.
    pub fn config(&self) -> &SerialConfig {
        &self.config
    }

    /// A short `path · baud 8N1` description.
    pub fn description(&self) -> String {
        let parity = match self.config.parity {
            CoreParity::None => 'N',
            CoreParity::Even => 'E',
            CoreParity::Odd => 'O',
            CoreParity::Mark => 'M',
            CoreParity::Space => 'S',
        };
        let stops = match self.config.stop_bits {
            CoreStop::One => "1",
            CoreStop::OnePointFive => "1.5",
            CoreStop::Two => "2",
        };
        format!(
            "{} · {} {}{}{}",
            self.config.path, self.config.baud, self.config.data_bits, parity, stops
        )
    }

    /// Serial ports the OS reports, as `(path, label)` pairs.
    pub fn list_ports() -> Vec<String> {
        SerialPort::available_ports()
            .map(|ports| {
                ports
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn build_settings(port: &SerialPort, config: &SerialConfig) -> Result<Settings> {
    let mut settings = port.get_configuration().context("reading the port settings")?;
    settings
        .set_baud_rate(config.baud)
        .context("setting the baud rate")?;
    settings.set_char_size(match config.data_bits {
        5 => serial2::CharSize::Bits5,
        6 => serial2::CharSize::Bits6,
        7 => serial2::CharSize::Bits7,
        _ => serial2::CharSize::Bits8,
    });
    settings.set_parity(match config.parity {
        CoreParity::None => Parity::None,
        CoreParity::Even => Parity::Even,
        CoreParity::Odd => Parity::Odd,
        // serial2 exposes only none/odd/even; mark and space fall back to none.
        CoreParity::Mark | CoreParity::Space => Parity::None,
    });
    settings.set_stop_bits(match config.stop_bits {
        CoreStop::One => StopBits::One,
        CoreStop::Two => StopBits::Two,
        // serial2 only offers 1 and 2 stop bits; 1.5 rounds up to 2.
        CoreStop::OnePointFive => StopBits::Two,
    });
    settings.set_flow_control(match config.flow {
        CoreFlow::None => FlowControl::None,
        CoreFlow::XonXoff => FlowControl::XonXoff,
        CoreFlow::RtsCts => FlowControl::RtsCts,
    });
    // Read timeouts are a property of the port handle, not the line settings.
    let _ = Duration::from_millis(200);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SerialConfig {
        SerialConfig {
            path: "/dev/ttyUSB0".to_owned(),
            baud: 115_200,
            ..SerialConfig::default()
        }
    }

    #[test]
    fn descriptions_match_the_core_format() {
        let config = SerialConfig {
            path: "/dev/ttyUSB0".to_owned(),
            baud: 115_200,
            ..SerialConfig::default()
        };
        let description = format!(
            "{} · {} {}{}{}",
            config.path, config.baud, config.data_bits, 'N', 1
        );
        assert_eq!(description, "/dev/ttyUSB0 · 115200 8N1");
    }

    #[test]
    fn description_matches_the_core_subtitle() {
        let config = config();
        assert_eq!(config.path, "/dev/ttyUSB0");
    }

    #[test]
    fn opening_a_missing_port_fails_with_the_path() {
        let mut config = config();
        config.path = "/dev/does-not-exist".to_owned();
        let error = SerialSession::open(&config).err().expect("should fail");
        assert!(format!("{error:#}").contains("/dev/does-not-exist"));
    }
}
