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
use wassh_ui::widgets::{password_input, select, small_button, small_button_enabled, text_input};

use super::protocol::{ProviderConfig, Protocol, default_providers};
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

/// A model-list fetch running on its own thread.
type ModelFetch = (usize, Receiver<Result<Vec<String>, String>>);

/// State the provider editor keeps between frames.
#[derive(Default)]
pub struct ProviderEditor {
    fetch: Option<ModelFetch>,
    /// Result of the last fetch: the message, and whether it succeeded.
    message: Option<(bool, String)>,
}

impl ProviderEditor {
    /// Draw the editor: the provider list goes into `values`, the API keys into `secrets`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        values: &mut Map<String, Value>,
        secrets: &mut PluginSecretDrafts,
    ) {
        let mut providers = read::<Vec<ProviderConfig>>(values, PROVIDERS_KEY)
            .filter(|providers| !providers.is_empty())
            .unwrap_or_else(default_providers);
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
        if small_button(ui, palette, "+ Add provider") {
            providers.push(custom_provider(&providers));
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

/// A new provider with an id no other provider uses.
fn custom_provider(providers: &[ProviderConfig]) -> ProviderConfig {
    let mut number = providers.len() + 1;
    while providers
        .iter()
        .any(|provider| provider.id == format!("custom-{number}"))
    {
        number += 1;
    }
    ProviderConfig {
        id: format!("custom-{number}"),
        name: "Custom provider".to_owned(),
        protocol: Protocol::Openai,
        base_url: String::new(),
        models: Vec::new(),
        embedding_model: None,
    }
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
        .filter(|providers| !providers.is_empty())
        .unwrap_or_else(default_providers)
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
        let mut secrets = PluginSecretDrafts::new();
        let texts = painted(&mut values, &mut secrets);
        for field in ["Protocol", "Base URL", "Models", "API key"] {
            assert!(texts.iter().any(|text| text == field), "{field} missing: {texts:?}");
        }
        assert!(texts.iter().any(|text| text == "Ollama"), "provider missing: {texts:?}");
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
}

