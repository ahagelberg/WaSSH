//! Remote folders as local drives: the Mounts pane.
//!
//! The pane only draws and asks. The app owns the connections, the filesystem behind each
//! drive and the list of remembered mounts, so a drive outlives the tab it was started from
//! and every tab shows the same list.

use std::cell::RefCell;
use std::rc::Rc;

use egui::Ui;
use wassh_ui::to_color32;

use crate::plugin::{
    DriverState, MountState, MountedDrive, Plugin, PluginContext, RemoteHost, RemoteMounts,
    RememberedMount,
};
use crate::{PluginId, PluginManifest};

/// Key the pane keeps its host choice under.
const HOST_KEY: &str = "host";
/// Key the pane keeps the last remote path under.
const PATH_KEY: &str = "path";
/// Key the pane keeps the last mount point under.
const POINT_KEY: &str = "mountPoint";

/// What the pane remembers while it is open.
#[derive(Default)]
pub struct MountsPane {
    /// First free drive letter, chosen once so it does not jump while the user types.
    suggested: Option<String>,
}

impl MountsPane {
    fn text(palette: &wassh_core::Palette, text: &str, size: f32) -> egui::RichText {
        egui::RichText::new(text)
            .size(size)
            .color(to_color32(palette.text))
    }

    fn muted(palette: &wassh_core::Palette, text: &str) -> egui::RichText {
        egui::RichText::new(text)
            .size(11.0)
            .color(to_color32(palette.text_muted))
    }

    /// How one mount's state reads.
    fn state_text(state: &MountState) -> String {
        match state {
            MountState::Idle => "not mounted".to_owned(),
            MountState::Mounting => "mounting…".to_owned(),
            MountState::NeedsPassword => "needs a password".to_owned(),
            MountState::Open => "mounted".to_owned(),
            MountState::Lost => "reconnecting…".to_owned(),
            MountState::Failed(reason) => reason.clone(),
        }
    }

    fn state_health(state: &MountState) -> wassh_core::HeaderHealth {
        match state {
            MountState::Open => wassh_core::HeaderHealth::Ok,
            MountState::Failed(_) => wassh_core::HeaderHealth::Bad,
            _ => wassh_core::HeaderHealth::Warn,
        }
    }

    /// The hosts the picker offers, with the tab's own host first.
    fn pickable_hosts(hosts: Vec<RemoteHost>, scope: &str) -> Vec<RemoteHost> {
        let mut hosts = hosts;
        if let Some(at) = hosts.iter().position(|host| host.id == scope) {
            let own = hosts.remove(at);
            hosts.insert(0, own);
        }
        hosts
    }
}

impl Plugin for MountsPane {
    fn manifest(&self) -> PluginManifest {
        PluginId::Mounts.manifest()
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        let Some(mounts) = context.mounts.clone() else {
            ui.label(Self::muted(
                &palette,
                "Mounting is not available in this window.",
            ));
            return;
        };
        let driver = mounts.borrow().driver();
        let drives = mounts.borrow().mounts();
        let remembered = mounts.borrow().remembered();
        let hosts = Self::pickable_hosts(mounts.borrow().hosts().to_vec(), &context.scope);
        let free = mounts.borrow().free_drives();
        let ready = driver.is_ready();

        ui.horizontal(|ui| {
            ui.label(Self::text(&palette, "Remote folders as drives", 13.0));
            ui.label(Self::muted(
                &palette,
                if ready { "WinFsp ready" } else { "needs WinFsp" },
            ));
        });

        if let DriverState::Missing(reason) = &driver {
            ui.label(Self::muted(&palette, reason));
            if ui.button("Install WinFsp").clicked() {
                mounts.borrow_mut().install_driver();
            }
            ui.separator();
        }

        self.draw_form(ui, context, &hosts, &free, ready);

        ui.separator();
        if drives.is_empty() {
            ui.label(Self::muted(
                &palette,
                "No folder is mounted. Pick a host and a folder above to get a drive.",
            ));
        }
        for drive in &drives {
            self.draw_mount(ui, context, drive, &mounts);
        }
        if !remembered.is_empty() {
            ui.separator();
            ui.label(Self::muted(&palette, "Remembered"));
            for entry in &remembered {
                self.draw_remembered(ui, context, entry, &mounts);
            }
        }
    }
}

impl MountsPane {
    /// The row that starts a mount.
    fn draw_form(
        &mut self,
        ui: &mut Ui,
        context: &mut PluginContext<'_>,
        hosts: &[RemoteHost],
        free: &[String],
        ready: bool,
    ) {
        let palette = context.palette;
        if hosts.is_empty() {
            ui.label(Self::muted(
                &palette,
                "Save a host first, then its folders can be mounted.",
            ));
            return;
        }
        if self.suggested.is_none() {
            self.suggested = free.first().cloned();
        }
        let mut host_id = context.text(HOST_KEY).unwrap_or_default().to_owned();
        if !hosts.iter().any(|host| host.id == host_id) {
            host_id = hosts[0].id.clone();
        }
        let mut path = context.text(PATH_KEY).unwrap_or("/").to_owned();
        let mut point = context
            .text(POINT_KEY)
            .map(str::to_owned)
            .or_else(|| self.suggested.clone())
            .unwrap_or_default();

        egui::Grid::new("mounts-form")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(Self::muted(&palette, "Host"));
                egui::ComboBox::from_id_salt("mounts-host")
                    .selected_text(
                        hosts
                            .iter()
                            .find(|host| host.id == host_id)
                            .map_or("choose a host", |host| host.name.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for host in hosts {
                            let label = match &host.problem {
                                Some(problem) => format!("{} ({problem})", host.name),
                                None => host.name.clone(),
                            };
                            ui.selectable_value(&mut host_id, host.id.clone(), label);
                        }
                    });
                ui.end_row();

                ui.label(Self::muted(&palette, "Folder"));
                ui.add(
                    egui::TextEdit::singleline(&mut path)
                        .hint_text("/srv/www")
                        .desired_width(220.0),
                );
                ui.end_row();

                ui.label(Self::muted(&palette, "Mount at"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut point)
                            .hint_text("X: or C:\\mnt\\web")
                            .desired_width(150.0),
                    );
                    egui::ComboBox::from_id_salt("mounts-point")
                        .selected_text("free")
                        .show_ui(ui, |ui| {
                            for letter in free {
                                ui.selectable_value(&mut point, letter.clone(), letter.clone());
                            }
                        });
                });
                ui.end_row();
            });

        ui.horizontal(|ui| {
            let enabled = ready && !point.trim().is_empty() && !path.trim().is_empty();
            if ui.add_enabled(enabled, egui::Button::new("Mount")).clicked() {
                let key = mount_key(&host_id, path.trim());
                mounts_of(context).borrow_mut().mount(
                    &key,
                    &host_id,
                    path.trim(),
                    point.trim(),
                );
            }
            if !ready {
                ui.label(Self::muted(&palette, "Install WinFsp first."));
            }
        });
        context.set_text(HOST_KEY, host_id);
        context.set_text(PATH_KEY, path);
        context.set_text(POINT_KEY, point);
    }

    /// One mounted folder, with what can be done to it.
    fn draw_mount(
        &mut self,
        ui: &mut Ui,
        context: &mut PluginContext<'_>,
        drive: &MountedDrive,
        mounts: &Rc<RefCell<dyn RemoteMounts>>,
    ) {
        let palette = context.palette;
        let colour = match Self::state_health(&drive.state) {
            wassh_core::HeaderHealth::Ok => palette.ok,
            wassh_core::HeaderHealth::Bad => palette.danger,
            _ => palette.warn,
        };
        ui.horizontal(|ui| {
            ui.label(Self::text(&palette, &drive.mount_point, 12.0));
            ui.label(Self::muted(
                &palette,
                &format!("{} on {}", drive.remote_path, drive.host_name),
            ));
            ui.label(
                egui::RichText::new(Self::state_text(&drive.state))
                    .size(11.0)
                    .color(to_color32(colour)),
            );
            if drive.reconnects > 0 && drive.state.is_open() {
                ui.label(Self::muted(
                    &palette,
                    &format!("reconnected {}x", drive.reconnects),
                ));
            }
        });
        ui.horizontal(|ui| {
            if drive.state.is_open() && ui.button("Open").clicked() {
                mounts.borrow_mut().reveal(&drive.mount_point);
            }
            if ui.button("Unmount").clicked() {
                mounts.borrow_mut().unmount(&drive.key);
            }
            if matches!(drive.state, MountState::Failed(_) | MountState::Lost)
                && ui.button("Retry").clicked()
            {
                mounts.borrow_mut().retry(&drive.key);
            }
        });
        if matches!(drive.state, MountState::NeedsPassword) {
            // The typed value lives in this frame only: it is handed over and forgotten.
            let mut typed = String::new();
            ui.horizontal(|ui| {
                ui.label(Self::muted(&palette, "Password"));
                let entry = ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .password(true)
                        .desired_width(160.0),
                );
                let entered =
                    entry.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if (entered || ui.button("Connect").clicked()) && !typed.is_empty() {
                    mounts.borrow_mut().supply_password(&drive.key, &typed);
                }
            });
        }
        ui.add_space(2.0);
    }

    /// One mount from an earlier run, offered again.
    fn draw_remembered(
        &mut self,
        ui: &mut Ui,
        context: &mut PluginContext<'_>,
        entry: &RememberedMount,
        mounts: &Rc<RefCell<dyn RemoteMounts>>,
    ) {
        let palette = context.palette;
        ui.horizontal(|ui| {
            ui.label(Self::text(&palette, &entry.mount_point, 12.0));
            ui.label(Self::muted(
                &palette,
                &format!("{} on {}", entry.remote_path, entry.host_name),
            ));
            if let Some(problem) = &entry.problem {
                ui.label(
                    egui::RichText::new(problem)
                        .size(11.0)
                        .color(to_color32(palette.warn)),
                );
            }
            if ui.button("Remount").clicked() {
                mounts.borrow_mut().remount(&entry.key);
            }
            if ui.button("Forget").clicked() {
                mounts.borrow_mut().forget(&entry.key);
            }
        });
    }
}

/// The key a mount is filed under: one host and one folder.
pub fn mount_key(host_id: &str, remote_path: &str) -> String {
    format!("{host_id}\u{1}{remote_path}")
}

/// The mounts behind a pane, for the places that only have the context.
fn mounts_of(context: &PluginContext<'_>) -> Rc<RefCell<dyn RemoteMounts>> {
    context.mounts.clone().expect("mounts are offered here")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::HeaderHealth;

    fn host(id: &str) -> RemoteHost {
        RemoteHost {
            id: id.to_owned(),
            name: id.to_owned(),
            address: format!("{id}:22"),
            ..RemoteHost::default()
        }
    }

    #[test]
    fn the_tabs_own_host_is_offered_first() {
        let hosts = MountsPane::pickable_hosts(vec![host("a"), host("b"), host("c")], "c");
        let ids: Vec<_> = hosts.iter().map(|host| host.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn a_tab_that_is_not_a_saved_host_leaves_the_order_alone() {
        let hosts = MountsPane::pickable_hosts(vec![host("a"), host("b")], "tab:7");
        let ids: Vec<_> = hosts.iter().map(|host| host.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn state_text_reads_as_a_person_expects() {
        assert_eq!(MountsPane::state_text(&MountState::Open), "mounted");
        assert_eq!(MountsPane::state_text(&MountState::Lost), "reconnecting…");
        assert_eq!(
            MountsPane::state_text(&MountState::Failed("no such host".to_owned())),
            "no such host"
        );
    }

    #[test]
    fn health_follows_the_state() {
        assert_eq!(MountsPane::state_health(&MountState::Open), HeaderHealth::Ok);
        assert_eq!(
            MountsPane::state_health(&MountState::Failed("x".to_owned())),
            HeaderHealth::Bad
        );
        assert_eq!(
            MountsPane::state_health(&MountState::Mounting),
            HeaderHealth::Warn
        );
    }

    #[test]
    fn a_mount_key_names_one_host_and_one_folder() {
        assert_eq!(mount_key("web-1", "/srv/www"), "web-1\u{1}/srv/www");
        assert_ne!(mount_key("web-1", "/srv"), mount_key("web-2", "/srv"));
    }
}
