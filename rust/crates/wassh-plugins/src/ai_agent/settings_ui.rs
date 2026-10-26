//! App-wide AI agent settings: the provider list.
//!
//! The pane only picks a provider and a model per conversation; endpoints and API
//! keys are configured once, in Options → AI agent, like the reference app's
//! dedicated settings view.

use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;

use egui::Ui;
use serde_json::{Map, Value};
use wassh_core::Palette;
use wassh_ui::widgets::{
    footer, modal, password_input, select, small_button, small_button_enabled, text_input,
};

use super::protocol::{
    CUSTOM_PROVIDER_PREFIX, PROVIDER_CUSTOM, PROVIDER_TYPES, ProviderConfig, ProviderType,
    Protocol,
};
use super::providers::provider_models;
use crate::color::to_color32;
use crate::settings::{PluginSecretDrafts, SecretDraft, secret_field_key};
use crate::PluginId;

/// Key the provider list is stored under.
pub const PROVIDERS_KEY: &str = "providers";
/// Key earlier builds kept the plain-text API keys under, kept for the migration.
pub const API_KEYS_KEY: &str = "apiKeys";
/// Prefix of the vault field one provider's key is filed under.
const SECRET_PREFIX: &str = "provider.";
/// Text shown in place of a stored key.
const STORED_PLACEHOLDER: &str = "(stored)";
/// Label size inside the editor.
const LABEL_SIZE: f32 = 10.5;
/// Width of the caption column inside a provider card.
const LABEL_WIDTH: f32 = 70.0;
/// Corner radius of a provider card.
const CARD_RADIUS: u8 = 6;
/// Padding inside a provider card.
const CARD_PADDING: i8 = 8;
/// Width kept free for the buttons next to a provider name.
const BUTTONS_WIDTH: f32 = 230.0;
/// Size of a provider's name in the list.
const NAME_SIZE: f32 = 12.5;
/// Height reserved for one control; a zero height would clip its text.
const CONTROL_HEIGHT: f32 = 22.0;
/// Width of the "add a provider" dialog.
const ADD_DIALOG_WIDTH: f32 = 460.0;
/// Size of the hint that the list is still empty.
const HINT_SIZE: f32 = 11.0;

/// A model-list fetch running on its own thread.
type ModelFetch = (usize, Receiver<Result<Vec<String>, String>>);

/// What the provider dialog produced when it was saved.
enum ProviderSave {
    /// A new provider, with the key typed for it (empty when it needs none).
    Add(ProviderConfig, String),
    /// An existing provider changed in place.
    Edit {
        index: usize,
        provider: ProviderConfig,
        /// A key typed to replace the stored one; empty keeps it.
        key: String,
        /// Drop the stored key instead of keeping it.
        forget_key: bool,
    },
}

/// The provider the dialog is filling in: a new one, or an existing one being edited.
struct NewProvider {
    /// Index of the provider being edited; `None` while adding a new one.
    editing: Option<usize>,
    /// Drop the stored API key when saving an edit.
    forget_key: bool,
    /// Index into [`PROVIDER_TYPES`].
    kind: usize,
    /// Name, editable only for the type the user describes.
    name: String,
    /// Wire format, editable only for that type.
    protocol: Protocol,
    /// Endpoint, editable for a hosted-on-your-own-machine or unknown service.
    base_url: String,
    /// The API key as typed; it is sealed into the vault when the provider is saved.
    key: String,
}

impl NewProvider {
    /// A draft for the first type in the list.
    fn new() -> Self {
        let kind = PROVIDER_TYPES[0];
        Self {
            editing: None,
            forget_key: false,
            kind: 0,
            name: kind.name.to_owned(),
            protocol: kind.protocol,
            base_url: kind.base_url.to_owned(),
            key: String::new(),
        }
    }

    /// A draft prefilled from an existing provider.
    ///
    /// The type is matched by endpoint so a hosted service shows its own name; anything
    /// else starts on the free-form type, with every field already filled in.
    fn edit(index: usize, provider: &ProviderConfig) -> Self {
        let kind = PROVIDER_TYPES
            .iter()
            .position(|kind| kind.id != PROVIDER_CUSTOM && kind.base_url == provider.base_url)
            .or_else(|| PROVIDER_TYPES.iter().position(|kind| kind.id == PROVIDER_CUSTOM))
            .unwrap_or(0);
        Self {
            editing: Some(index),
            forget_key: false,
            kind,
            name: provider.name.clone(),
            protocol: provider.protocol,
            base_url: provider.base_url.clone(),
            key: String::new(),
        }
    }

    /// Whether the dialog is changing a provider rather than adding one.
    fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// The type the dropdown shows.
    fn selected(&self) -> ProviderType {
        PROVIDER_TYPES[self.kind.min(PROVIDER_TYPES.len() - 1)]
    }

    /// Point the draft at another type, taking its endpoint and wire format.
    fn select_type(&mut self, kind: usize) {
        self.kind = kind;
        let selected = self.selected();
        self.name = selected.name.to_owned();
        self.protocol = selected.protocol;
        self.base_url = selected.base_url.to_owned();
    }

    /// Whether the draft can be saved: every provider needs a name and an endpoint.
    fn is_complete(&self) -> bool {
        let kind = self.selected();
        if !self.is_editing() && kind.id != PROVIDER_CUSTOM {
            return true;
        }
        !self.name.trim().is_empty() && !self.base_url.trim().is_empty()
    }

    /// The edited provider, the key typed for it, and whether to drop the stored key.
    ///
    /// The id is kept, so the API key already sealed in the vault stays bound to it.
    fn into_edit(self, existing: &ProviderConfig) -> (ProviderConfig, String, bool) {
        let base_url = self.base_url.trim().to_owned();
        let provider = ProviderConfig {
            id: existing.id.clone(),
            name: self.name.trim().to_owned(),
            protocol: self.protocol,
            // A new endpoint means the old model list belongs to another service.
            models: if base_url == existing.base_url {
                existing.models.clone()
            } else {
                Vec::new()
            },
            embedding_model: existing.embedding_model.clone(),
            base_url,
        };
        (provider, self.key.trim().to_owned(), self.forget_key)
    }

    /// The provider this draft describes, and the key to seal for it.
    fn into_provider(self, providers: &[ProviderConfig]) -> (ProviderConfig, String) {
        let kind = self.selected();
        let custom = kind.id == PROVIDER_CUSTOM;
        let base = if custom {
            CUSTOM_PROVIDER_PREFIX.trim_end_matches('-')
        } else {
            kind.id
        };
        let provider = ProviderConfig {
            id: unique_id(base, providers),
            name: if custom {
                self.name.trim().to_owned()
            } else {
                kind.name.to_owned()
            },
            protocol: if custom { self.protocol } else { kind.protocol },
            base_url: if kind.has_fixed_endpoint() {
                kind.base_url.to_owned()
            } else {
                self.base_url.trim().to_owned()
            },
            // The model list is fetched from the provider once it is in use.
            models: Vec::new(),
            embedding_model: None,
        };
        (provider, self.key.trim().to_owned())
    }
}

/// State the provider editor keeps between frames.
#[derive(Default)]
pub struct ProviderEditor {
    fetch: Option<ModelFetch>,
    /// Result of the last fetch: the message, and whether it succeeded.
    message: Option<(bool, String)>,
    /// The add dialog, while it is open.
    new_provider: Option<NewProvider>,
}

impl ProviderEditor {
    /// Whether the "add or edit a provider" dialog is open on top of the list.
    pub fn dialog_open(&self) -> bool {
        self.new_provider.is_some()
    }

    /// Draw the editor: the provider list goes into `values`, the API keys into `secrets`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        values: &mut Map<String, Value>,
        secrets: &mut PluginSecretDrafts,
    ) {
        let mut providers = read::<Vec<ProviderConfig>>(values, PROVIDERS_KEY).unwrap_or_default();
        // Keys for the model fetch, read from the vault drafts.
        let keys: BTreeMap<String, String> = providers
            .iter()
            .filter_map(|provider| {
                let draft = secrets.get(&secret_key(&provider.id))?;
                (!draft.draft.is_empty()).then(|| (provider.id.clone(), draft.draft.clone()))
            })
            .collect();

        self.poll_fetch(&mut providers);

        let muted = to_color32(palette.text_muted);
        let mut remove = None;
        let mut fetch = None;
        let mut edit = None;
        let count = providers.len();
        for (index, provider) in providers.iter_mut().enumerate() {
            let card = egui::Frame::new()
                .fill(to_color32(palette.bg_panel_2))
                .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(egui::CornerRadius::same(CARD_RADIUS))
                .inner_margin(egui::Margin::same(CARD_PADDING));
            card.show(ui, |ui| {
                ui.horizontal(|ui| {
                    let room = (ui.available_width() - BUTTONS_WIDTH).max(80.0);
                    slot(ui, room, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&provider.name)
                                    .size(NAME_SIZE)
                                    .strong()
                                    .color(to_color32(palette.text)),
                            )
                            .truncate(),
                        );
                    });
                    if small_button(ui, palette, "Edit") {
                        edit = Some(index);
                    }
                    if small_button_enabled(ui, palette, "Fetch models", self.fetch.is_none()) {
                        fetch = Some(index);
                    }
                    if small_button_enabled(ui, palette, "Remove", count > 1) {
                        remove = Some(index);
                    }
                });
                // Just the models: everything else is read in the Edit dialog.
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(models_text(&provider.models))
                            .size(LABEL_SIZE + 1.0)
                            .color(to_color32(palette.text_muted)),
                    )
                    .truncate(),
                );
            });
            ui.add_space(4.0);
        }

        if let Some(index) = remove {
            let removed = providers.remove(index);
            // Forget the key too, so removing a provider leaves nothing in the vault.
            let draft: &mut SecretDraft = secrets.entry(secret_key(&removed.id)).or_default();
            draft.clear = true;
            draft.draft.clear();
        }
        if providers.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No providers yet. Add the one you want to chat through; its API key \
                     goes into the credential vault.",
                )
                .size(HINT_SIZE)
                .color(muted),
            );
        }
        match self.provider_dialog(ui, palette, &providers, secrets) {
            Some(ProviderSave::Add(provider, key)) => {
                if !key.is_empty() {
                    let draft: &mut SecretDraft =
                        secrets.entry(secret_key(&provider.id)).or_default();
                    draft.draft = key;
                    draft.clear = false;
                }
                providers.push(provider);
            }
            Some(ProviderSave::Edit {
                index,
                provider,
                key,
                forget_key,
            }) => {
                let draft: &mut SecretDraft =
                    secrets.entry(secret_key(&provider.id)).or_default();
                if !key.is_empty() {
                    draft.draft = key;
                    draft.clear = false;
                } else if forget_key {
                    draft.clear = true;
                    draft.draft.clear();
                }
                if let Some(slot) = providers.get_mut(index) {
                    *slot = provider;
                }
            }
            None => {}
        }
        if let Some(index) = edit
            && self.new_provider.is_none()
            && let Some(provider) = providers.get(index)
        {
            self.new_provider = Some(NewProvider::edit(index, provider));
        }
        if let Some(index) = fetch {
            self.start_fetch(&providers, &keys, index);
        }
        if let Some((ok, text)) = &self.message {
            let colour = if *ok {
                to_color32(palette.ok)
            } else {
                to_color32(palette.danger)
            };
            ui.add(
                egui::Label::new(egui::RichText::new(text.as_str()).size(11.5).color(colour)).wrap(),
            );
        }

        values.insert(
            PROVIDERS_KEY.to_owned(),
            serde_json::to_value(&providers).unwrap_or(Value::Null),
        );
    }

    /// The "+ Add provider" button and the dialog it opens.
    ///
    /// The same dialog edits a provider when the card's Edit button opens it: the
    /// provider list itself is read-only, so every change goes through here.
    ///
    /// Nothing is configured out of the box, so the dialog starts with the type: OpenAI,
    /// Anthropic, DeepSeek, Grok, Open Router, a local Ollama, or an endpoint of the
    /// user's own. The type fixes the wire format and, for hosted services, the endpoint.
    fn provider_dialog(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        providers: &[ProviderConfig],
        secrets: &PluginSecretDrafts,
    ) -> Option<ProviderSave> {
        if self.new_provider.is_none() {
            if small_button(ui, palette, "+ Add provider") {
                self.new_provider = Some(NewProvider::new());
            }
            return None;
        }
        let mut draft = self.new_provider.take()?;
        // Whether the provider being edited already has a key in the vault.
        let stored_key = draft
            .editing
            .and_then(|index| providers.get(index))
            .is_some_and(|provider| {
                secrets
                    .get(&secret_key(&provider.id))
                    .is_some_and(|draft| draft.stored)
            });
        let editing = draft.is_editing();
        let mut save = false;
        let mut cancel = false;
        let response = modal(
            ui.ctx(),
            "ai-agent-add-provider",
            palette,
            if editing { "Edit provider" } else { "Add a provider" },
            ADD_DIALOG_WIDTH,
            |ui| {
                let muted = to_color32(palette.text_muted);
                let options: Vec<(usize, &str)> = PROVIDER_TYPES
                    .iter()
                    .enumerate()
                    .map(|(index, kind)| (index, kind.name))
                    .collect();
                caption(ui, muted, "Provider", |ui| {
                    if let Some(kind) = select(ui, "ai-new-provider-type", &draft.kind, &options)
                    {
                        draft.select_type(kind);
                    }
                });
                let kind = draft.selected();
                // A hosted service knows its endpoint; a local or unknown one does not.
                // An existing provider shows every field, whatever its type was.
                if kind.has_fixed_endpoint() && !editing {
                    caption(ui, muted, "Endpoint", |ui| {
                        ui.label(
                            egui::RichText::new(kind.base_url)
                                .size(LABEL_SIZE)
                                .color(to_color32(palette.text)),
                        );
                    });
                } else {
                    caption(ui, muted, "Base URL", |ui| {
                        text_input(ui, palette, &mut draft.base_url, "https://host/v1");
                    });
                }
                if kind.id == PROVIDER_CUSTOM || editing {
                    caption(ui, muted, "Name", |ui| {
                        text_input(ui, palette, &mut draft.name, "My provider");
                    });
                    caption(ui, muted, "Protocol", |ui| {
                        let options = [
                            (Protocol::Openai, "OpenAI compatible"),
                            (Protocol::Anthropic, "Anthropic"),
                        ];
                        if let Some(protocol) =
                            select(ui, "ai-new-provider-protocol", &draft.protocol, &options)
                        {
                            draft.protocol = protocol;
                        }
                    });
                }
                if kind.needs_key {
                    caption(ui, muted, "API key", |ui| {
                        let hint = if stored_key && draft.key.is_empty() {
                            STORED_PLACEHOLDER
                        } else {
                            "paste the provider key"
                        };
                        password_input(ui, palette, &mut draft.key, hint);
                    });
                    if stored_key && draft.key.is_empty() {
                        ui.add_space(2.0);
                        // The drafts are not `SecretDraft`s here, so a plain flag will do:
                        // saving reads it, and the dialog is dropped afterwards.
                        wassh_ui::widgets::checkbox(
                            ui,
                            palette,
                            &mut draft.forget_key,
                            "Forget the stored key",
                        );
                    }
                } else {
                    ui.label(
                        egui::RichText::new("This service runs without an API key.")
                            .size(HINT_SIZE)
                            .color(muted),
                    );
                }
                footer(ui, |ui| {
                    let label = if editing { "Save" } else { "Add" };
                    if small_button_enabled(ui, palette, label, draft.is_complete()) {
                        save = true;
                    }
                    if small_button(ui, palette, "Cancel") {
                        cancel = true;
                    }
                });
            },
        );
        if save {
            return Some(match draft.editing {
                Some(index) => {
                    let existing = providers.get(index)?.clone();
                    let (provider, key, forget_key) = draft.into_edit(&existing);
                    ProviderSave::Edit {
                        index,
                        provider,
                        key,
                        forget_key,
                    }
                }
                None => {
                    let (provider, key) = draft.into_provider(providers);
                    ProviderSave::Add(provider, key)
                }
            });
        }
        if !(response.dismissed || cancel) {
            self.new_provider = Some(draft);
        }
        None
    }

    /// Ask a provider for its model list on a worker thread.
    fn start_fetch(
        &mut self,
        providers: &[ProviderConfig],
        keys: &BTreeMap<String, String>,
        index: usize,
    ) {
        let Some(provider) = providers.get(index).cloned() else {
            return;
        };
        let key = keys.get(&provider.id).cloned().unwrap_or_default();
        let name = provider.name.clone();
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(provider_models(&provider, &key));
        });
        self.fetch = Some((index, events));
        self.message = Some((true, format!("Reading {name}'s models…")));
    }

    /// Collect a finished model list.
    fn poll_fetch(&mut self, providers: &mut [ProviderConfig]) {
        let Some((index, events)) = &self.fetch else {
            return;
        };
        match events.try_recv() {
            Ok(Ok(models)) if !models.is_empty() => {
                let index = *index;
                let count = models.len();
                if let Some(provider) = providers.get_mut(index) {
                    provider.models = models;
                }
                self.message = Some((true, format!("{count} models available.")));
                self.fetch = None;
            }
            Ok(Ok(_)) => {
                self.message = Some((false, "The provider listed no models.".to_owned()));
                self.fetch = None;
            }
            Ok(Err(error)) => {
                self.message = Some((false, error));
                self.fetch = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.fetch = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
}

/// An id no configured provider uses, so a second OpenAI is still its own entry.
fn unique_id(base: &str, providers: &[ProviderConfig]) -> String {
    if !providers.iter().any(|provider| provider.id == base) {
        return base.to_owned();
    }
    let mut number = 2;
    while providers
        .iter()
        .any(|provider| provider.id == format!("{base}-{number}"))
    {
        number += 1;
    }
    format!("{base}-{number}")
}

/// Read one key of `values` as `T`.
fn read<T: serde::de::DeserializeOwned>(values: &Map<String, Value>, key: &str) -> Option<T> {
    serde_json::from_value(values.get(key)?.clone()).ok()
}

/// Vault field one provider's API key is filed under.
pub fn provider_secret_field(provider_id: &str) -> String {
    format!("{SECRET_PREFIX}{provider_id}")
}

/// Ids of the providers the app-wide settings list.
pub fn provider_ids(values: &Map<String, Value>) -> Vec<String> {
    read::<Vec<ProviderConfig>>(values, PROVIDERS_KEY)
        .unwrap_or_default()
        .into_iter()
        .map(|provider| provider.id)
        .collect()
}

/// Whether a vault field names a provider's API key rather than a schema field.
///
/// Provider fields are app-wide, so a write to one must not follow a host.
pub fn is_provider_key_field(field: &str) -> bool {
    field.starts_with(SECRET_PREFIX)
}

/// Draft key for one provider's API key.
fn secret_key(provider_id: &str) -> String {
    secret_field_key(PluginId::AiAgent.as_str(), &provider_secret_field(provider_id))
}

/// A provider's models, or a hint when it has none yet.
fn models_text(models: &[String]) -> String {
    if models.is_empty() {
        "none listed yet".to_owned()
    } else {
        models.join(", ")
    }
}

/// A muted caption followed by a control that fills the rest of the card.
fn caption(ui: &mut Ui, muted: egui::Color32, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let room = (ui.available_width() - LABEL_WIDTH).max(60.0);
        ui.allocate_ui_with_layout(
            egui::Vec2::new(LABEL_WIDTH, CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new(label).size(LABEL_SIZE).color(muted));
            },
        );
        slot(ui, room, add);
    });
}

/// A control that fills `room` pixels, including fields that ask for all they can get.
fn slot(ui: &mut Ui, room: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        egui::Vec2::new(room, CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(room);
            add(ui);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text the editor painted, after one settled frame.
    fn painted(values: &mut Map<String, Value>, secrets: &mut PluginSecretDrafts) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut editor = ProviderEditor::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(800.0, 900.0),
            )),
            ..Default::default()
        };
        let mut texts = Vec::new();
        for _ in 0..2 {
            let output = ctx.run_ui(input.clone(), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(800.0)
                    .show(ui, |ui| editor.show(ui, &Palette::dark(), values, secrets));
            });
            texts.clear();
            for clipped in &output.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    texts.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
        }
        texts
    }

    #[test]
    fn a_provider_card_shows_its_name_and_models_only() {
        let mut values = Map::new();
        values.insert(
            PROVIDERS_KEY.to_owned(),
            serde_json::json!([{ "id": "anthropic", "name": "Anthropic", "protocol": "anthropic",
                                 "baseUrl": "https://api.anthropic.com", "models": [] }]),
        );
        let mut secrets = PluginSecretDrafts::new();
        let texts = painted(&mut values, &mut secrets);
        assert!(texts.iter().any(|text| text == "Anthropic"), "provider missing: {texts:?}");
        assert!(
            texts.iter().any(|text| text == "none listed yet"),
            "an empty model list still says so: {texts:?}"
        );
        // Everything else is only in the Edit dialog, not on the card.
        for hidden in ["Protocol", "Base URL", "API key", "https://api.anthropic.com"] {
            assert!(
                !texts.iter().any(|text| text == hidden),
                "{hidden:?} belongs in the Edit dialog: {texts:?}"
            );
        }
        // The provider list is a setting; the keys never are.
        assert!(values.contains_key(PROVIDERS_KEY));
        assert!(
            !values.contains_key(API_KEYS_KEY),
            "a key must never be written to the settings map"
        );
    }

    /// One provider, the way the list stores it.
    fn ollama() -> Value {
        serde_json::json!([{ "id": "ollama", "name": "Ollama", "protocol": "openai",
                             "baseUrl": "http://127.0.0.1:11434/v1",
                             "models": ["llama3.1"] }])
    }

    /// Paint the editor and collect every text with its position.
    ///
    /// `events` go to the last of `passes` frames: an `Area`'s (and so a modal's) first
    /// frame is a sizing pass that paints nothing, so click in one call and read in the
    /// next.
    fn painted_at(
        values: &mut Map<String, Value>,
        secrets: &mut PluginSecretDrafts,
        editor: &mut ProviderEditor,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        passes: usize,
    ) -> Vec<(i32, i32, String)> {
        let mut rows = Vec::new();
        for pass in 0..passes.max(1) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(800.0, 900.0),
                )),
                events: if pass + 1 == passes {
                    events.clone()
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(800.0)
                    .show(ui, |ui| editor.show(ui, &Palette::dark(), values, secrets));
            });
            let shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
            rows = shapes
                .into_iter()
                .filter_map(|clipped| match clipped.shape {
                    egui::Shape::Text(text) => Some((
                        text.pos.y as i32,
                        text.pos.x as i32,
                        text.galley.text().to_owned(),
                    )),
                    _ => None,
                })
                .collect();
            rows.sort();
        }
        rows
    }

    /// A press and release in the middle of a painted text.
    fn click_at(row: (i32, i32, String)) -> Vec<egui::Event> {
        let pos = egui::pos2(row.1 as f32 + 4.0, row.0 as f32 + 4.0);
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ]
    }

    #[test]
    fn a_provider_card_is_a_read_only_summary_with_an_edit_button() {
        let mut values = Map::new();
        values.insert(PROVIDERS_KEY.to_owned(), ollama());
        let mut secrets = PluginSecretDrafts::new();
        let mut editor = ProviderEditor::default();
        let ctx = egui::Context::default();
        let rows = painted_at(&mut values, &mut secrets, &mut editor, &ctx, Vec::new(), 2);
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        // The title, its models, and the buttons — nothing else.
        for text in ["Ollama", "llama3.1"] {
            assert!(texts.contains(&text), "{text:?} missing from the card: {texts:?}");
        }
        for hidden in [
            "Protocol",
            "Base URL",
            "API key",
            "OpenAI compatible",
            "http://127.0.0.1:11434/v1",
            "not set",
        ] {
            assert!(
                !texts.contains(&hidden),
                "{hidden:?} belongs in the Edit dialog: {texts:?}"
            );
        }
        // The card is read-only: its button is the way in.
        assert!(texts.contains(&"Edit"), "no Edit button: {texts:?}");
        assert!(!editor.dialog_open(), "drawing the list opens no dialog");
        assert_eq!(values[PROVIDERS_KEY], ollama(), "nothing was written back");
    }

    #[test]
    fn the_edit_button_opens_the_dialog_in_edit_mode() {
        let mut values = Map::new();
        values.insert(PROVIDERS_KEY.to_owned(), ollama());
        let mut secrets = PluginSecretDrafts::new();
        let mut editor = ProviderEditor::default();
        let ctx = egui::Context::default();
        let rows = painted_at(&mut values, &mut secrets, &mut editor, &ctx, Vec::new(), 2);
        let edit = rows
            .iter()
            .find(|(_, _, text)| text == "Edit")
            .cloned()
            .expect("the card has an Edit button");
        painted_at(&mut values, &mut secrets, &mut editor, &ctx, click_at(edit), 2);
        assert!(editor.dialog_open(), "Edit opens the provider dialog");
        // The dialog paints on its next frame, since it was created after the click.
        let rows = painted_at(&mut values, &mut secrets, &mut editor, &ctx, Vec::new(), 2);
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(
            texts.contains(&"Edit provider"),
            "it opens in edit mode: {texts:?}"
        );
        assert!(texts.contains(&"Save"), "and offers Save: {texts:?}");
        assert!(!texts.contains(&"Add a provider"), "not the add dialog");
        // Its fields start on the provider's own values.
        assert!(texts.contains(&"Ollama"), "the name is prefilled: {texts:?}");
        assert!(
            texts.contains(&"http://127.0.0.1:11434/v1"),
            "the endpoint is prefilled: {texts:?}"
        );
    }

    #[test]
    fn saving_an_edit_keeps_the_id_and_models_and_leaves_an_untouched_key() {
        let existing = ProviderConfig {
            id: "ollama".to_owned(),
            name: "Ollama".to_owned(),
            protocol: Protocol::Openai,
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            models: vec!["llama3.1".to_owned()],
            embedding_model: None,
        };
        let draft = NewProvider::edit(0, &existing);
        assert!(draft.is_editing());
        assert_eq!(draft.name, "Ollama");
        assert_eq!(draft.base_url, existing.base_url);
        // The type is matched by endpoint, so the card edits as the service it is.
        assert!(PROVIDER_TYPES[draft.kind].name.starts_with("Ollama"));
        let (provider, key, forget) = draft.into_edit(&existing);
        assert_eq!(provider.id, "ollama", "the vault key stays bound to it");
        assert_eq!(provider.models, ["llama3.1"]);
        assert!(key.is_empty() && !forget, "an untouched key is left alone");

        // A new endpoint drops the old model list, and the typed key replaces the stored one.
        let mut draft = NewProvider::edit(0, &existing);
        draft.base_url = "https://other.example/v1".to_owned();
        draft.key = "  sk-new  ".to_owned();
        let (provider, key, _) = draft.into_edit(&existing);
        assert!(provider.models.is_empty());
        assert_eq!(provider.base_url, "https://other.example/v1");
        assert_eq!(key, "sk-new", "trimmed");
    }

    #[test]
    fn stored_providers_round_trip() {
        let mut values = Map::new();
        values.insert(
            PROVIDERS_KEY.to_owned(),
            serde_json::json!([
                { "id": "a", "name": "A", "protocol": "openai", "baseUrl": "https://a", "models": [] },
                { "id": "b", "name": "B", "protocol": "openai", "baseUrl": "https://b", "models": [] }
            ]),
        );
        let mut secrets = PluginSecretDrafts::new();
        secrets.insert(
            secret_key("b"),
            SecretDraft {
                stored: true,
                ..SecretDraft::default()
            },
        );
        painted(&mut values, &mut secrets);
        let providers = values[PROVIDERS_KEY].as_array().expect("list");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[1]["name"], "B");
        // The stored key stays in the vault drafts, untouched.
        assert!(secrets[&secret_key("b")].stored);
        assert!(!secrets[&secret_key("b")].clear);
    }

    #[test]
    fn provider_keys_use_the_vault_field_name() {
        assert_eq!(secret_key("deepseek"), "ai-agent|provider.deepseek");
    }

    #[test]
    fn nothing_is_configured_out_of_the_box() {
        let mut values = Map::new();
        let mut secrets = PluginSecretDrafts::new();
        let texts = painted(&mut values, &mut secrets);
        assert!(
            texts.iter().any(|text| text.contains("No providers yet")),
            "the empty list needs a hint: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "+ Add provider"),
            "the add button is missing: {texts:?}"
        );
        assert_eq!(
            values[PROVIDERS_KEY],
            serde_json::json!([]),
            "an empty list is stored, not a list of defaults"
        );
        assert!(provider_ids(&values).is_empty());
    }

    #[test]
    fn the_dialog_starts_on_a_known_service_and_asks_for_its_key() {
        let draft = NewProvider::new();
        let kind = draft.selected();
        assert_eq!(kind.id, "openai");
        assert_eq!(draft.base_url, "https://api.openai.com/v1");
        assert_eq!(draft.protocol, Protocol::Openai);
        assert!(kind.needs_key);
        assert!(draft.is_complete(), "a hosted service only needs the key");

        // The endpoint is the service's, so adding keeps it whatever was typed.
        let (provider, _) = draft.into_provider(&[]);
        assert_eq!(provider.id, "openai");
        assert_eq!(provider.name, "OpenAI");
        assert_eq!(provider.base_url, "https://api.openai.com/v1");
        assert_eq!(provider.models, Vec::<String>::new(), "fetched on first use");
    }

    #[test]
    fn the_selected_type_decides_which_fields_matter() {
        let custom = PROVIDER_TYPES
            .iter()
            .position(|kind| kind.id == PROVIDER_CUSTOM)
            .expect("custom");
        let mut draft = NewProvider::new();
        draft.select_type(custom);
        assert!(draft.base_url.is_empty(), "a described provider has no endpoint yet");
        assert!(!draft.is_complete(), "the endpoint is still missing");
        draft.base_url = "https://models.internal/v1".to_owned();
        draft.name = "  In-house  ".to_owned();
        draft.protocol = Protocol::Anthropic;
        assert!(draft.is_complete());
        let (provider, _) = draft.into_provider(&[]);
        assert_eq!(provider.id, "custom");
        assert_eq!(provider.name, "In-house");
        assert_eq!(provider.protocol, Protocol::Anthropic);
        assert_eq!(provider.base_url, "https://models.internal/v1");

        // A local Ollama server needs no key and its port is editable.
        let ollama = PROVIDER_TYPES
            .iter()
            .position(|kind| kind.id == "ollama")
            .expect("ollama");
        let mut draft = NewProvider::new();
        draft.select_type(ollama);
        assert!(!draft.selected().needs_key);
        assert!(!draft.selected().has_fixed_endpoint());
        draft.base_url = "http://127.0.0.1:11500/v1".to_owned();
        assert_eq!(draft.into_provider(&[]).0.base_url, "http://127.0.0.1:11500/v1");
    }

    #[test]
    fn adding_the_same_service_twice_keeps_the_keys_apart() {
        let (first, _) = NewProvider::new().into_provider(&[]);
        let (second, _) = NewProvider::new().into_provider(std::slice::from_ref(&first));
        assert_eq!(first.id, "openai");
        assert_eq!(second.id, "openai-2", "the id is also the vault field");
        assert_eq!(unique_id("openai", &[first, second.clone()]), "openai-3");
    }
}

