//! The quick-connect form pinned under the sidebar.
//!
//! The quick-connect form: a type selector, then protocol-specific fields, and a
//! Connect button. Nothing is saved unless the user later parks the session as a host.

use wassh_core::{ConnectionParams, ConnectionType, Palette};
use wassh_ui::widgets::{Button, field, password_input, select, text_input};

/// Draft state for the form.
pub struct QuickForm {
    /// Protocol to connect with.
    pub kind: ConnectionType,
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
}

impl Default for QuickForm {
    fn default() -> Self {
        Self {
            kind: ConnectionType::Ssh,
            host: String::new(),
            port: "22".to_owned(),
            username: String::new(),
            password: String::new(),
            serial_path: "/dev/ttyUSB0".to_owned(),
            baud: 115_200,
            ports: Vec::new(),
        }
    }
}

impl QuickForm {
    /// Switch protocol, resetting the port to that protocol's default.
    pub fn set_kind(&mut self, kind: ConnectionType) {
        if self.kind != kind {
            self.kind = kind;
            if kind != ConnectionType::Serial && kind != ConnectionType::Local {
                self.port = kind.default_port().to_string();
            }
            if kind == ConnectionType::Serial {
                self.refresh_ports();
            }
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

    /// Whether the form has enough information to attempt a connection.
    pub fn is_complete(&self) -> bool {
        match self.kind {
            ConnectionType::Ssh | ConnectionType::Telnet => !self.host.trim().is_empty(),
            ConnectionType::Serial => !self.serial_path.trim().is_empty(),
            ConnectionType::Local => true,
        }
    }

    /// Build connection parameters from the form.
    pub fn params(&self) -> ConnectionParams {
        let mut params = ConnectionParams {
            kind: self.kind,
            ..ConnectionParams::default()
        };
        match self.kind {
            ConnectionType::Ssh | ConnectionType::Telnet => {
                params.host = self.host.trim().to_owned();
                params.port = self.port.trim().parse().unwrap_or(self.kind.default_port());
                params.username = self.username.trim().to_owned();
            }
            ConnectionType::Serial => {
                params.serial.path = self.serial_path.trim().to_owned();
                params.serial.baud = self.baud;
            }
            ConnectionType::Local => {}
        }
        params.normalized()
    }
}

/// Draw the form; returns connection parameters once the user submits.
pub fn quick_connect(ui: &mut egui::Ui, palette: &Palette, form: &mut QuickForm) -> Option<ConnectionParams> {
    let mut submitted = false;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("QUICK CONNECT")
                .size(10.0)
                .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
        );
    });
    ui.add_space(4.0);

    let options = [
        (ConnectionType::Ssh, "SSH"),
        (ConnectionType::Telnet, "Telnet"),
        (ConnectionType::Serial, "Serial"),
        (ConnectionType::Local, "Local"),
    ];
    if let Some(kind) = select(ui, "quick-kind", &form.kind, &options) {
        form.set_kind(kind);
    }

    match form.kind {
        ConnectionType::Ssh | ConnectionType::Telnet => {
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
        ConnectionType::Serial => {
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
        ConnectionType::Local => {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Starts $SHELL as a login shell.")
                    .size(11.0)
                    .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
            );
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

    if submitted && enabled {
        Some(form.params())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_protocol_updates_the_default_port() {
        let mut form = QuickForm::default();
        assert_eq!(form.port, "22");
        form.set_kind(ConnectionType::Telnet);
        assert_eq!(form.port, "23");
        form.set_kind(ConnectionType::Serial);
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
    fn local_sessions_are_always_complete() {
        let mut form = QuickForm::default();
        form.set_kind(ConnectionType::Local);
        assert!(form.is_complete());
    }

    #[test]
    fn serial_params_use_the_device_and_baud() {
        let mut form = QuickForm::default();
        form.set_kind(ConnectionType::Serial);
        form.serial_path = "/dev/ttyACM0".to_owned();
        form.baud = 9_600;
        let params = form.params();
        assert_eq!(params.serial.path, "/dev/ttyACM0");
        assert_eq!(params.serial.baud, 9_600);
    }
}
