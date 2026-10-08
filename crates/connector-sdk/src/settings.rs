//! Bounded, non-secret declarative settings shared by plugins and their host.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
pub const MAX_SETTINGS_FIELDS: usize = 64;
pub const MAX_SETTINGS_TEXT_CHARS: usize = 4096;
pub const SETTINGS_DESCRIBE: &str = "settings.describe/v1";
pub const SETTINGS_VALIDATE: &str = "settings.validate/v1";
pub const SETTINGS_COMPILE: &str = "settings.compile/v1";
pub const SETTINGS_MIGRATE: &str = "settings.migrate/v1";

pub type LocalizedText = BTreeMap<String, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsDescriptor {
    pub schema_version: u32,
    pub title: LocalizedText,
    pub fields: Vec<PluginSettingsField>,
    pub defaults: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(try_from = "Value")]
pub struct PluginSettingsField {
    pub key: String,
    pub label: LocalizedText,
    #[serde(default)]
    pub description: LocalizedText,
    pub required: bool,
    #[serde(flatten)]
    pub kind: PluginSettingsFieldKind,
}

impl TryFrom<Value> for PluginSettingsField {
    type Error = &'static str;

    fn try_from(mut value: Value) -> Result<Self, Self::Error> {
        let object = value.as_object_mut().ok_or("field must be an object")?;
        let allowed: &[&str] = match object.get("type").and_then(Value::as_str) {
            Some("string") => &["type", "max_length"],
            Some("boolean") => &["type"],
            Some("integer") => &["type", "minimum", "maximum"],
            Some("enum") => &["type", "options"],
            _ => return Err("unsupported field type"),
        };
        if object.keys().any(|key| {
            !["key", "label", "description", "required"].contains(&key.as_str())
                && !allowed.contains(&key.as_str())
        }) {
            return Err("unknown field attribute");
        }
        let key = serde_json::from_value(object.remove("key").ok_or("missing key")?)
            .map_err(|_| "invalid key")?;
        let label = serde_json::from_value(object.remove("label").ok_or("missing label")?)
            .map_err(|_| "invalid label")?;
        let description = object
            .remove("description")
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "invalid description")?
            .unwrap_or_default();
        let required = object
            .remove("required")
            .and_then(|value| value.as_bool())
            .ok_or("missing required")?;
        let kind = serde_json::from_value(value).map_err(|_| "invalid field type")?;
        Ok(Self {
            key,
            label,
            description,
            required,
            kind,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginSettingsFieldKind {
    String { max_length: usize },
    Boolean,
    Integer { minimum: i64, maximum: i64 },
    Enum { options: Vec<PluginSettingsOption> },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsOption {
    pub value: String,
    pub label: LocalizedText,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsDocument {
    pub schema_version: u32,
    pub values: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsValidation {
    pub valid: bool,
    pub errors: Vec<PluginSettingsFieldError>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsFieldError {
    pub field: String,
    pub code: String,
}

impl PluginSettingsDescriptor {
    pub fn validate_descriptor(&self) -> bool {
        if self.schema_version == 0
            || self.fields.len() > MAX_SETTINGS_FIELDS
            || !valid_text(&self.title, true)
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_SETTINGS_BYTES)
        {
            return false;
        }
        let mut keys = HashSet::new();
        self.fields.iter().all(|field| {
            valid_key(&field.key)
                && keys.insert(&field.key)
                && valid_text(&field.label, true)
                && valid_text(&field.description, false)
                && match &field.kind {
                    PluginSettingsFieldKind::String { max_length } => {
                        (1..=MAX_SETTINGS_TEXT_CHARS).contains(max_length)
                    }
                    PluginSettingsFieldKind::Boolean => true,
                    PluginSettingsFieldKind::Integer { minimum, maximum } => {
                        const JS_SAFE: i64 = 9_007_199_254_740_991;
                        minimum <= maximum && *minimum >= -JS_SAFE && *maximum <= JS_SAFE
                    }
                    PluginSettingsFieldKind::Enum { options } => {
                        let mut seen = HashSet::new();
                        !options.is_empty()
                            && options.len() <= MAX_SETTINGS_FIELDS
                            && options.iter().all(|option| {
                                !option.value.is_empty()
                                    && option.value.chars().count() <= 256
                                    && !option.value.chars().any(char::is_control)
                                    && seen.insert(&option.value)
                                    && valid_text(&option.label, true)
                            })
                    }
                }
        }) && self.validate_values(&self.defaults)
    }

    pub fn validate_values(&self, values: &Value) -> bool {
        let Some(values) = values.as_object() else {
            return false;
        };
        if serde_json::to_vec(values).map_or(true, |bytes| bytes.len() > MAX_SETTINGS_BYTES)
            || values
                .keys()
                .any(|key| !self.fields.iter().any(|field| &field.key == key))
        {
            return false;
        }
        self.fields.iter().all(|field| {
            let Some(value) = values.get(&field.key) else {
                return !field.required;
            };
            match &field.kind {
                PluginSettingsFieldKind::String { max_length } => {
                    value.as_str().is_some_and(|value| {
                        value.chars().count() <= *max_length && !value.chars().any(char::is_control)
                    })
                }
                PluginSettingsFieldKind::Boolean => value.is_boolean(),
                PluginSettingsFieldKind::Integer { minimum, maximum } => value
                    .as_i64()
                    .is_some_and(|value| (*minimum..=*maximum).contains(&value)),
                PluginSettingsFieldKind::Enum { options } => value
                    .as_str()
                    .is_some_and(|value| options.iter().any(|option| option.value == value)),
            }
        })
    }

    pub fn default_document(&self) -> PluginSettingsDocument {
        PluginSettingsDocument {
            schema_version: self.schema_version,
            values: self.defaults.clone(),
        }
    }
}

fn valid_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && value.as_bytes()[0].is_ascii_alphabetic()
}

fn valid_text(value: &LocalizedText, required: bool) -> bool {
    (!required || !value.is_empty())
        && value.len() <= 8
        && value.iter().all(|(language, text)| {
            !language.is_empty()
                && language.len() <= 32
                && language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !text.is_empty()
                && text.chars().count() <= MAX_SETTINGS_TEXT_CHARS
                && !text.chars().any(char::is_control)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn descriptor_and_values_are_bounded_and_fail_closed() {
        let mut descriptor: PluginSettingsDescriptor = serde_json::from_value(json!({
            "schema_version":1,"title":{"en":"Example"},"fields":[
                {"key":"mode","label":{"en":"Mode"},"required":true,"type":"enum",
                 "options":[{"value":"safe","label":{"en":"Safe"}}]}
            ],"defaults":{"mode":"safe"}
        }))
        .unwrap();
        assert!(descriptor.validate_descriptor());
        assert!(!descriptor.validate_values(&json!({"mode":"safe","unknown":1})));
        assert!(!descriptor.validate_values(&json!({"mode":"unsafe"})));
        assert!(!descriptor.validate_values(&json!({})));
        descriptor.fields.push(descriptor.fields[0].clone());
        assert!(!descriptor.validate_descriptor());
    }

    #[test]
    fn fields_reject_executable_and_secret_extensions() {
        for kind in ["boolean", "string"] {
            let field = json!({
                "key":"flag","label":{"en":"Flag"},"required":true,
                "type":kind,"max_length":32,"secret":true,"script":"execute"
            });
            assert!(serde_json::from_value::<PluginSettingsField>(field).is_err());
        }
    }
}
