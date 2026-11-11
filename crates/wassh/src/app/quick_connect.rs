//! The quick-connect form pinned under the sidebar.
//!
//! The quick-connect form: a type selector, then protocol-specific fields, and a
//! Connect button. Nothing is saved unless the user later parks the session as a host.

use wassh_core::{ConnectionParams, ConnectionType, LocalShell, Palette};
use wassh_ui::widgets::{Button, field, password_input, select, small_button, text_input};

/// Label for the plain local shell: PowerShell on Windows, `$SHELL` elsewhere.
const LOCAL_LABEL: &str = if cfg!(windows) {
    "Local (PS)"
} else {
    "Local"
};
/// Label for the Windows command prompt.
const CMD_LABEL: &str = "Local (cmd)";
/// Label for a WSL environment.
const WSL_LABEL: &str = "Local (WSL)";
/// Height the form takes in the sidebar.
const QUICK_HEIGHT: f32 = 236.0;
/// Height the form takes when the WSL environment picker is shown.
const QUICK_HEIGHT_WSL: f32 = 290.0;
/// Opens the installer instead of picking an existing environment.
const NEW_ENV_LABEL: &str = "New environment…";
/// Opens the dialog that lists and removes environments.
const MANAGE_LABEL: &str = "Manage environments…";
/// Shown as the picker's value while no distribution is selected.
const DEFAULT_DISTRO_LABEL: &str = "Default distribution";

/// What the form connects to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickKind {
    Ssh,
    Telnet,
    Serial,
    /// The platform shell: `$SHELL` on Unix, PowerShell on Windows.
    Local,
    /// The Windows command prompt.
    Cmd,
    /// A WSL environment.
    Wsl,
}

impl QuickKind {
    /// The transport a session for this kind uses.
    fn connection_type(self) -> ConnectionType {
        match self {
            Self::Ssh => ConnectionType::Ssh,
            Self::Telnet => ConnectionType::Telnet,
            Self::Serial => ConnectionType::Serial,
            Self::Local | Self::Cmd | Self::Wsl => ConnectionType::Local,
        }
    }
}

/// The selectable kinds; nothing but the platform shell exists off Windows.
fn kind_options() -> Vec<(QuickKind, &'static str)> {
    let mut options = vec![
        (QuickKind::Ssh, "SSH"),
        (QuickKind::Telnet, "Telnet"),
        (QuickKind::Serial, "Serial"),
        (QuickKind::Local, LOCAL_LABEL),
    ];
    if cfg!(windows) {
        options.push((QuickKind::Cmd, CMD_LABEL));
        options.push((QuickKind::Wsl, WSL_LABEL));
    }
    options
}

/// What the form asked the shell to do.
pub enum QuickAction {
    /// Open a session with these parameters.
    Connect(Box<ConnectionParams>),
    /// Open the dialog that installs a WSL environment.
    InstallWsl,
    /// Open the dialog that lists and removes WSL environments.
    ManageWsl,
}

/// Draft state for the form.
pub struct QuickForm {
    /// What to connect to.
    pub kind: QuickKind,
    /// Hostname or IP address.
    pub host: String,
    /// Port, kept as text so it can be cleared while typing.
    pub port: String,
    /// Login name.
    pub username: String,
    /// Password (never persisted here).
    pub password: String,
    /// Serial device path.
    pub serial_path: String,
    /// Serial baud rate.
    pub baud: u32,
    /// Serial devices the OS reported the last time the form was opened.
    pub ports: Vec<String>,
    /// WSL distribution to run; `None` asks WSL for its default.
    pub wsl_distro: Option<String>,
    /// WSL distributions the OS reported the last time the picker was opened.
    pub wsl_distros: Vec<String>,
}

impl Default for QuickForm {
    fn default() -> Self {
        Self {
            kind: QuickKind::Ssh,
            host: String::new(),
            port: "22".to_owned(),
            username: String::new(),
            password: String::new(),
            serial_path: "/dev/ttyUSB0".to_owned(),
            baud: 115_200,
            ports: Vec::new(),
            wsl_distro: None,
            wsl_distros: Vec::new(),
        }
    }
}

impl QuickForm {
    fn set_kind(&mut self, kind: QuickKind) {
        if self.kind == kind {
            return;
        }
        self.kind = kind;
        if matches!(kind, QuickKind::Ssh | QuickKind::Telnet) {
            self.port = kind.connection_type().default_port().to_string();
        }
        if kind == QuickKind::Serial {
            self.refresh_ports();
        }
        if kind == QuickKind::Wsl {
            self.refresh_wsl();
        }
    }

    /// Re-read the serial devices the OS knows about.
    pub fn refresh_ports(&mut self) {
        self.ports = wassh_serial::SerialSession::list_ports();
        if let Some(first) = self.ports.first()
            && !self.ports.contains(&self.serial_path)
            && self.serial_path == "/dev/ttyUSB0"
        {
            self.serial_path = first.clone();
        }
    }

    /// Re-read the installed WSL distributions, keeping the selection when it survives.
    pub fn refresh_wsl(&mut self) {
        self.wsl_distros = wassh_term::wsl::distros();
        if self
            .wsl_distro
            .as_ref()
            .is_none_or(|distro| !self.wsl_distros.contains(distro))
        {
            self.wsl_distro = self.wsl_distros.first().cloned();
        }
    }

    /// Height the form needs in the sidebar.
    pub fn height(&self) -> f32 {
        if self.kind == QuickKind::Wsl {
            QUICK_HEIGHT_WSL
        } else {
            QUICK_HEIGHT
        }
    }

    /// Whether the form has enough information to attempt a connection.
    pub fn is_complete(&self) -> bool {
        match self.kind {
            QuickKind::Ssh | QuickKind::Telnet => !self.host.trim().is_empty(),
            QuickKind::Serial => !self.serial_path.trim().is_empty(),
            QuickKind::Local | QuickKind::Cmd => true,
            QuickKind::Wsl => !self.wsl_distros.is_empty(),
        }
    }

    /// Build connection parameters from the form.
    pub fn params(&self) -> ConnectionParams {
        let mut params = ConnectionParams {
            kind: self.kind.connection_type(),
            ..ConnectionParams::default()
        };
        match self.kind {
            QuickKind::Ssh | QuickKind::Telnet => {
                params.host = self.host.trim().to_owned();
                params.port = self
                    .port
                    .trim()
                    .parse()
                    .unwrap_or(params.kind.default_port());
                params.username = self.username.trim().to_owned();
            }
            QuickKind::Serial => {
                params.serial.path = self.serial_path.trim().to_owned();
                params.serial.baud = self.baud;
            }
            QuickKind::Local => {}
            QuickKind::Cmd => params.local = LocalShell::Cmd,
            QuickKind::Wsl => {
                params.local = LocalShell::Wsl {
                    distro: self.wsl_distro.clone(),
                };
            }
        }
        params.normalized()
    }
}

/// Draw the form; returns what the user asked for once they act on it.
pub fn quick_connect(ui: &mut egui::Ui, palette: &Palette, form: &mut QuickForm) -> Option<QuickAction> {
    let mut submitted = false;
    let mut install = false;
    let mut manage = false;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("QUICK CONNECT")
                .size(10.0)
                .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
        );
    });
    ui.add_space(4.0);

    let options = kind_options();
    if let Some(kind) = select(ui, "quick-kind", &form.kind, &options) {
        form.set_kind(kind);
    }

    match form.kind {
        QuickKind::Ssh | QuickKind::Telnet => {
            ui.add_space(6.0);
            let response = text_input(ui, palette, &mut form.host, "host or IP");
            submitted |= response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let port_width = 56.0;
                ui.scope(|ui| {
                    ui.set_width(port_width);
                    text_input(ui, palette, &mut form.port, "22");
                });
                ui.add_space(4.0);
                text_input(ui, palette, &mut form.username, "username");
            });
            ui.add_space(4.0);
            password_input(ui, palette, &mut form.password, "password");
        }
        QuickKind::Serial => {
            ui.add_space(6.0);
            if form.ports.is_empty() {
                text_input(ui, palette, &mut form.serial_path, "/dev/ttyUSB0");
            } else {
                let current = form.serial_path.clone();
                egui::ComboBox::from_id_salt("quick-serial-port")
                    .selected_text(
                        egui::RichText::new(current.clone())
                            .size(12.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text)),
                    )
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for port in &form.ports {
                            if ui
                                .selectable_label(*port == current, port)
                                .clicked()
                            {
                                form.serial_path = port.clone();
                            }
                        }
                    });
            }
            ui.add_space(4.0);
            let mut baud = form.baud;
            field(ui, palette, "Baud", None, |ui| {
                wassh_ui::widgets::number_input(ui, palette, &mut baud, 300, 3_000_000);
                ui.response()
            });
            form.baud = baud;
        }
        QuickKind::Local => {
            ui.add_space(6.0);
            hint(
                ui,
                palette,
                if cfg!(windows) {
                    "Starts PowerShell."
                } else {
                    "Starts $SHELL as a login shell."
                },
            );
        }
        QuickKind::Cmd => {
            ui.add_space(6.0);
            hint(ui, palette, "Starts the Windows command prompt.");
        }
        QuickKind::Wsl => {
            ui.add_space(6.0);
            if form.wsl_distros.is_empty() {
                hint(ui, palette, "No WSL environments found.");
            } else {
                let current = form.wsl_distro.clone();
                let shown = current.as_deref().unwrap_or(DEFAULT_DISTRO_LABEL);
                egui::ComboBox::from_id_salt("quick-wsl-distro")
                    .selected_text(
                        egui::RichText::new(shown)
                            .size(12.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text)),
                    )
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for distro in &form.wsl_distros {
                            if ui
                                .selectable_label(current.as_deref() == Some(distro.as_str()), distro)
                                .clicked()
                            {
                                form.wsl_distro = Some(distro.clone());
                            }
                        }
                    });
            }
            ui.add_space(4.0);
            if small_button(ui, palette, NEW_ENV_LABEL) {
                install = true;
            }
            ui.add_space(4.0);
            if small_button(ui, palette, MANAGE_LABEL) {
                manage = true;
            }
        }
    }

    ui.add_space(6.0);
    let enabled = form.is_complete();
    if Button::new("Connect")
        .primary()
        .enabled(enabled)
        .min_width(ui.available_width())
        .show(ui, palette)
        .clicked()
        && enabled
    {
        submitted = true;
    }
    ui.add_space(6.0);

    if install {
        Some(QuickAction::InstallWsl)
    } else if manage {
        Some(QuickAction::ManageWsl)
    } else if submitted && enabled {
        Some(QuickAction::Connect(Box::new(form.params())))
    } else {
        None
    }
}

fn hint(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_protocol_updates_the_default_port() {
        let mut form = QuickForm::default();
        assert_eq!(form.port, "22");
        form.set_kind(QuickKind::Telnet);
        assert_eq!(form.port, "23");
        form.set_kind(QuickKind::Serial);
        assert_eq!(form.port, "23", "serial has no port to update");
    }

    #[test]
    fn ssh_requires_a_host() {
        let mut form = QuickForm::default();
        assert!(!form.is_complete());
        form.host = "example.com".to_owned();
        assert!(form.is_complete());
        let params = form.params();
        assert_eq!(params.host, "example.com");
        assert_eq!(params.port, 22);
        assert_eq!(params.kind, ConnectionType::Ssh);
    }

    #[test]
    fn local_shells_are_always_complete() {
        let mut form = QuickForm::default();
        form.set_kind(QuickKind::Local);
        assert!(form.is_complete());
        form.set_kind(QuickKind::Cmd);
        assert!(form.is_complete());
    }

    #[test]
    fn local_kinds_map_to_the_connection_and_shell() {
        let mut form = QuickForm::default();
        form.set_kind(QuickKind::Local);
        let params = form.params();
        assert_eq!(params.kind, ConnectionType::Local);
        assert_eq!(params.local, LocalShell::System);

        form.kind = QuickKind::Cmd;
        assert_eq!(form.params().local, LocalShell::Cmd);

        form.kind = QuickKind::Wsl;
        assert!(!form.is_complete(), "no environment to run yet");
        form.wsl_distros = vec!["Ubuntu".to_owned()];
        form.wsl_distro = Some("Ubuntu".to_owned());
        assert!(form.is_complete());
        let params = form.params();
        assert_eq!(params.kind, ConnectionType::Local);
        assert_eq!(
            params.local,
            LocalShell::Wsl {
                distro: Some("Ubuntu".to_owned())
            }
        );
    }

    #[test]
    fn serial_params_use_the_device_and_baud() {
        let mut form = QuickForm::default();
        form.set_kind(QuickKind::Serial);
        form.serial_path = "/dev/ttyACM0".to_owned();
        form.baud = 9_600;
        let params = form.params();
        assert_eq!(params.serial.path, "/dev/ttyACM0");
        assert_eq!(params.serial.baud, 9_600);
    }
}
