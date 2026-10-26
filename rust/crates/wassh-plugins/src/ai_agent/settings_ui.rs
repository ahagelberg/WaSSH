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
/// Width kept free for the two buttons next to a provider name.
const BUTTONS_WIDTH: f32 = 170.0;
/// Height reserved for one control; a zero height would clip its text.
const CONTROL_HEIGHT: f32 = 22.0;
/// Width of the "add a provider" dialog.
const ADD_DIALOG_WIDTH: f32 = 460.0;
/// Size of the hint that the list is still empty.
const HINT_SIZE: f32 = 11.0;

/// A model-list fetch running on its own thread.
type ModelFetch = (usize, Receiver<Result<Vec<String>, String>>);

/// A provider the user is filling in before it is added.
struct NewProvider {
    /// Index into [`PROVIDER_TYPES`].
    kind: usize,
    /// Name, editable only for the type the user describes.
    name: String,
    /// Wire format, editable only for that type.
    protocol: Protocol,
    /// Endpoint, editable for a hosted-on-your-own-machine or unknown service.
    base_url: String,
    /// The API key as typed; it is sealed into the vault when the provider is added.
    key: String,
}

impl NewProvider {
    /// A draft for the first type in the list.
    fn new() -> Self {
        let kind = PROVIDER_TYPES[0];
        Self {
            kind: 0,
            name: kind.name.to_owned(),
            protocol: kind.protocol,
            base_url: kind.base_url.to_owned(),
            key: String::new(),
        }
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

    /// Whether the draft can be added: a described provider needs a name and an endpoint.
    fn is_complete(&self) -> bool {
        let kind = self.selected();
        if kind.id != PROVIDER_CUSTOM {
            return true;
        }
        !self.name.trim().is_empty() && !self.base_url.trim().is_empty()
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
    /// Whether the "add a provider" dialog is open on top of the editor.
    pub fn is_adding(&self) -> bool {
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
        let count = providers.len();
        for (index, provider) in providers.iter_mut().enumerate() {
            let card = egui::Frame::new()
                .fill(to_color32(palette.bg_panel_2))
                .stroke(egui::Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(egui::CornerRadius::same(CARD_RADIUS))
                .inner_margin(egui::Margin::same(CARD_PADDING));
            card.show(ui, |ui| {
                ui.horizontal(|ui| {
                    let room = (ui.available_width() - BUTTONS_WIDTH).max(120.0);
                    slot(ui, room, |ui| {
                        text_input(ui, palette, &mut provider.name, "name");
                    });
                    if small_button_enabled(ui, palette, "Fetch models", self.fetch.is_none()) {
                        fetch = Some(index);
                    }
                    if small_button_enabled(ui, palette, "Remove", count > 1) {
                        remove = Some(index);
                    }
                });
                caption(ui, muted, "Protocol", |ui| {
                    let options = [
                        (Protocol::Openai, "OpenAI compatible"),
                        (Protocol::Anthropic, "Anthropic"),
                    ];
                    if let Some(protocol) =
                        select(ui, ("ai-provider-protocol", index), &provider.protocol, &options)
                    {
                        provider.protocol = protocol;
                    }
                });
                caption(ui, muted, "Base URL", |ui| {
                    text_input(ui, palette, &mut provider.base_url, "https://host/v1");
                });
                caption(ui, muted, "Models", |ui| {
                    let mut models = provider.models.join(", ");
                    if text_input(ui, palette, &mut models, "model-a, model-b").changed() {
                        provider.models = models
                            .split(',')
                            .map(|model| model.trim().to_owned())
                            .filter(|model| !model.is_empty())
                            .collect();
                    }
                });
                caption(ui, muted, "API key", |ui| {
                    let draft = secrets.entry(secret_key(&provider.id)).or_default();
                    let hint = if draft.stored && draft.draft.is_empty() {
                        STORED_PLACEHOLDER
                    } else {
                        "paste the provider key"
                    };
                    if password_input(ui, palette, &mut draft.draft, hint).changed() {
                        draft.clear = false;
                    }
                    if draft.stored && !draft.clear && small_button(ui, palette, "Clear") {
                        draft.clear = true;
                        draft.draft.clear();
                    }
                });
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
        if let Some((provider, key)) = self.add_controls(ui, palette, &providers) {
            if !key.is_empty() {
                let draft: &mut SecretDraft = secrets.entry(secret_key(&provider.id)).or_default();
                draft.draft = key;
                draft.clear = false;
            }
            providers.push(provider);
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
    /// Nothing is configured out of the box, so the dialog starts with the type: OpenAI,
    /// Anthropic, DeepSeek, Grok, Open Router, a local Ollama, or an endpoint of the
    /// user's own. The type fixes the wire format and, for hosted services, the endpoint.
    fn add_controls(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        providers: &[ProviderConfig],
    ) -> Option<(ProviderConfig, String)> {
        if self.new_provider.is_none() {
            if small_button(ui, palette, "+ Add provider") {
                self.new_provider = Some(NewProvider::new());
            }
            return None;
        }
        let mut draft = self.new_provider.take()?;
        let mut add = false;
        let mut cancel = false;
        let response = modal(
            ui.ctx(),
            "ai-agent-add-provider",
            palette,
            "Add a provider",
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
                if kind.has_fixed_endpoint() {
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
                if kind.id == PROVIDER_CUSTOM {
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
                        password_input(ui, palette, &mut draft.key, "paste the provider key");
                    });
                } else {
                    ui.label(
                        egui::RichText::new("This service runs without an API key.")
                            .size(HINT_SIZE)
                            .color(muted),
                    );
                }
                footer(ui, |ui| {
                    if small_button_enabled(ui, palette, "Add", draft.is_complete()) {
                        add = true;
                    }
                    if small_button(ui, palette, "Cancel") {
                        cancel = true;
                    }
                });
            },
        );
        if add {
            return Some(draft.into_provider(providers));
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
    fn every_provider_lists_its_endpoint_models_and_key() {
        let mut values = Map::new();
        values.insert(
            PROVIDERS_KEY.to_owned(),
            serde_json::json!([{ "id": "anthropic", "name": "Anthropic", "protocol": "anthropic",
                                 "baseUrl": "https://api.anthropic.com", "models": [] }]),
        );
        let mut secrets = PluginSecretDrafts::new();
        let texts = painted(&mut values, &mut secrets);
        for field in ["Protocol", "Base URL", "Models", "API key"] {
            assert!(texts.iter().any(|text| text == field), "{field} missing: {texts:?}");
        }
        assert!(texts.iter().any(|text| text == "Anthropic"), "provider missing: {texts:?}");
        assert!(
            texts.iter().any(|text| text == "https://api.anthropic.com"),
            "base URL missing: {texts:?}"
        );
        // The provider list is a setting; the keys never are.
        assert!(values.contains_key(PROVIDERS_KEY));
        assert!(
            !values.contains_key(API_KEYS_KEY),
            "a key must never be written to the settings map"
        );
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

