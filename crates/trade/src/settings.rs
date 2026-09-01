//! The schema an adapter declares for its own settings, the values a user
//! supplies against it, and the rule that keeps credentials from leaving.
//!
//! # Why a schema and not a free-form blob
//!
//! Every adapter needs different settings — the simulator wants a starting
//! balance, an exchange wants an API key and secret, a broker wants a
//! server address and a login. The engine cannot know any of them, and a
//! plugin cannot be allowed to ship user interface code, so the plugin
//! declares *what it needs* as data and both sides build from that: the
//! server validates against it before writing a row, and the web client
//! renders a form from the same document. Neither side has adapter-specific
//! code in it.
//!
//! [`ActionForm`] reuses the same types, so an adapter's custom actions
//! ("deposit funds", "reset the account") get a form for free.
//!
//! # Secrets do not travel outward, by construction
//!
//! A [`SecretString`] serialises as `null`, always. Not "unless a flag is
//! set", not "the API layer remembers to strip it" — the [`serde::Serialize`]
//! impl has no path that writes the value, so an API response, a log line
//! built with `serde_json`, or a debug print cannot leak one even by
//! mistake. Persisting a secret needs [`SettingsValues::to_storage_json`],
//! a separate, explicitly named function the account store is the only
//! caller of.
//!
//! The matching input rule: a secret field that arrives **absent or null**
//! means *keep what is stored*, so a client that renders a form from
//! settings it was never shown the secrets of can still submit that form
//! without blanking the credential. [`SettingsInput::carry_secrets_from`]
//! is what applies it.

use std::collections::BTreeMap;
use std::fmt;

use senken_core::decimal::{Scaled, format_scaled, parse_scaled};
use serde::{Deserialize, Serialize};

/// Why a set of values did not satisfy a schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SettingsError {
    /// A required field was not supplied and has no default.
    #[error("`{0}` is required")]
    Missing(String),
    /// A value was supplied for a field the schema does not declare.
    ///
    /// Rejected rather than ignored: silently dropping a setting a user
    /// typed is indistinguishable, from their side, from the setting having
    /// no effect.
    #[error("`{0}` is not a setting this adapter has")]
    Unknown(String),
    /// The value's type does not match the field's kind.
    #[error("`{field}` expects {expected}")]
    WrongType {
        /// The field's key.
        field: String,
        /// What the field's kind accepts, in words.
        expected: String,
    },
    /// A numeric value fell outside the field's declared bounds.
    #[error("`{field}` must be between {min} and {max}")]
    OutOfRange {
        /// The field's key.
        field: String,
        /// The lowest accepted value, formatted at the field's scale.
        min: String,
        /// The highest accepted value, formatted at the field's scale.
        max: String,
    },
    /// A choice field was given a value not among its options.
    #[error("`{field}` must be one of: {options}")]
    NotAnOption {
        /// The field's key.
        field: String,
        /// The accepted values, comma separated.
        options: String,
    },
    /// A text value was longer than the field allows.
    #[error("`{field}` must be at most {max} characters")]
    TooLong {
        /// The field's key.
        field: String,
        /// The longest accepted length.
        max: usize,
    },
    /// The stored settings JSON could not be read back.
    #[error("stored settings are corrupt: {0}")]
    Corrupt(String),
}

/// A credential. Serialises as `null` and never as its contents.
///
/// See this module's docs: the guarantee is structural, not a convention
/// the API layer has to remember. `Debug` is redacted for the same reason —
/// a `tracing` field or a panic message must not become the leak.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a credential.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Reads the credential back.
    ///
    /// Named to be conspicuous at the call site: an adapter authenticating
    /// a request is the intended caller, anything that formats a response
    /// is not.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// `true` when a credential is actually stored — the one fact about a
    /// secret that is safe to report, and the one a settings form needs to
    /// show "configured" rather than an empty box.
    #[must_use]
    pub fn is_set(&self) -> bool {
        !self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_set() {
            "SecretString(set)"
        } else {
            "SecretString(unset)"
        })
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_none()
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(
            Option::<String>::deserialize(deserializer)?.unwrap_or_default(),
        ))
    }
}

/// One option of a [`FieldKind::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    /// The value stored when this option is picked.
    pub value: String,
    /// What the form shows for it.
    pub label: String,
}

impl ChoiceOption {
    /// An option whose stored value and label are given separately.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// What a settings field holds, and what a form should render for it.
///
/// A closed set, and deliberately a small one: every kind here has an
/// obvious control in a form and an unambiguous validation rule. An adapter
/// that wants something richer describes it with several of these rather
/// than the engine growing a general-purpose UI language — which is how a
/// settings schema turns into a plugin shipping markup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FieldKind {
    /// A single line of text.
    Text {
        /// Applied when the field is absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// Longest accepted length.
        max_len: usize,
        /// Ghost text for the input.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        placeholder: String,
    },
    /// A credential. Never reported back to a client; see
    /// [`SecretString`].
    Secret {
        /// Ghost text for the input.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        placeholder: String,
    },
    /// A whole number.
    Number {
        /// Applied when the field is absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<i64>,
        /// Lowest accepted value.
        min: i64,
        /// Highest accepted value.
        max: i64,
        /// A unit to show beside the input (`x`, `bps`, `ms`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        unit: String,
    },
    /// A fixed-point decimal, stored as `(scale, value)`.
    ///
    /// **Never an `f64`.** A starting balance, a fee rate and a leverage
    /// multiplier all end up in an arithmetic path that decides money, and
    /// this project's rule is that such a value is a scaled integer from
    /// the wire format through to storage. The form sends a decimal string;
    /// [`SettingsSchema::validate`] parses it at this scale, exactly, and
    /// refuses anything finer rather than rounding it silently.
    Decimal {
        /// Fractional digits the value is held at.
        scale: u8,
        /// Applied when the field is absent, at `scale`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<i64>,
        /// Lowest accepted value, at `scale`.
        min: i64,
        /// Highest accepted value, at `scale`.
        max: i64,
        /// A unit to show beside the input (`USD`, `%`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        unit: String,
    },
    /// An on/off switch.
    Toggle {
        /// Applied when the field is absent.
        default: bool,
    },
    /// One of a fixed list.
    Choice {
        /// Applied when the field is absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// The options, in display order.
        options: Vec<ChoiceOption>,
    },
}

/// One field of a schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingField {
    /// The key this field's value is stored under.
    pub key: String,
    /// The form's label.
    pub label: String,
    /// One line under the input explaining what it does. Product copy: it
    /// is shown to a user, so it never cites a plan or a ticket.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// Whether a value must be present after defaults are applied.
    pub required: bool,
    /// What the field holds.
    #[serde(flatten)]
    pub kind: FieldKind,
}

impl SettingField {
    /// A field with no help text, required.
    pub fn new(key: impl Into<String>, label: impl Into<String>, kind: FieldKind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            help: String::new(),
            required: true,
            kind,
        }
    }

    /// Adds the line of help shown under the input.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    /// Marks the field optional.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// `true` when this field holds a credential.
    #[must_use]
    pub fn is_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Secret { .. })
    }
}

/// An ordered set of fields an adapter declares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSchema {
    /// The fields, in the order a form should render them.
    pub fields: Vec<SettingField>,
}

/// A form an adapter action takes its parameters through — the same schema
/// type, named for where it is used.
pub type ActionForm = SettingsSchema;

/// One stored setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum SettingValue {
    /// A credential. Serialises as `null`.
    Secret(SecretString),
    /// An on/off switch.
    Toggle(bool),
    /// A whole number.
    Number(i64),
    /// A fixed-point decimal at a schema-declared scale.
    Decimal(Scaled),
    /// Text, or the value of a chosen option.
    Text(String),
}

impl SettingValue {
    /// The text of a [`Text`](Self::Text), or `None` for any other kind.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The boolean of a [`Toggle`](Self::Toggle), or `None`.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Toggle(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer of a [`Number`](Self::Number), or `None`.
    #[must_use]
    pub fn as_number(&self) -> Option<i64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// The scaled integer of a [`Decimal`](Self::Decimal), or `None`.
    #[must_use]
    pub fn as_decimal(&self) -> Option<Scaled> {
        match self {
            Self::Decimal(value) => Some(*value),
            _ => None,
        }
    }

    /// The credential of a [`Secret`](Self::Secret), or `None`.
    #[must_use]
    pub fn as_secret(&self) -> Option<&SecretString> {
        match self {
            Self::Secret(value) => Some(value),
            _ => None,
        }
    }
}

/// The values stored for one account, keyed by field.
///
/// Ordered so that two equal sets of settings serialise identically, which
/// is what lets a stored blob be compared without parsing it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettingsValues(BTreeMap<String, SettingValue>);

impl SettingsValues {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value stored under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.0.get(key)
    }

    /// Inserts or replaces one value.
    pub fn set(&mut self, key: impl Into<String>, value: SettingValue) {
        self.0.insert(key.into(), value);
    }

    /// `true` when nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every key with a value, in key order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Text stored under `key`.
    #[must_use]
    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(SettingValue::as_text)
    }

    /// A toggle stored under `key`.
    #[must_use]
    pub fn toggle(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(SettingValue::as_bool)
    }

    /// A whole number stored under `key`.
    #[must_use]
    pub fn number(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(SettingValue::as_number)
    }

    /// A fixed-point decimal stored under `key`.
    #[must_use]
    pub fn decimal(&self, key: &str) -> Option<Scaled> {
        self.get(key).and_then(SettingValue::as_decimal)
    }

    /// A credential stored under `key`.
    #[must_use]
    pub fn secret(&self, key: &str) -> Option<&SecretString> {
        self.get(key).and_then(SettingValue::as_secret)
    }

    /// Which secret fields of `schema` actually hold a credential.
    ///
    /// The only thing a client is ever told about a secret, and enough for
    /// a form to show "configured — leave blank to keep" instead of an
    /// empty box that looks like the credential was lost.
    #[must_use]
    pub fn secret_status(&self, schema: &SettingsSchema) -> BTreeMap<String, bool> {
        schema
            .fields
            .iter()
            .filter(|field| field.is_secret())
            .map(|field| {
                let set = self.secret(&field.key).is_some_and(SecretString::is_set);
                (field.key.clone(), set)
            })
            .collect()
    }

    /// Serialises **including** credentials, for the account store to
    /// write.
    ///
    /// The one function in this crate that writes a [`SecretString`]'s
    /// contents. Everything else — every API response, every log line —
    /// goes through the ordinary [`Serialize`] impl, which writes `null`.
    ///
    /// # Errors
    /// [`SettingsError::Corrupt`] if the values cannot be encoded, which
    /// `serde_json` only does for a map key that is not a string.
    pub fn to_storage_json(&self) -> Result<String, SettingsError> {
        let mirror: BTreeMap<&str, StoredValue<'_>> = self
            .0
            .iter()
            .map(|(key, value)| (key.as_str(), StoredValue::from(value)))
            .collect();
        serde_json::to_string(&mirror).map_err(|error| SettingsError::Corrupt(error.to_string()))
    }

    /// Reads back what [`to_storage_json`](Self::to_storage_json) wrote.
    ///
    /// # Errors
    /// [`SettingsError::Corrupt`] when the text is not the object this
    /// crate wrote.
    pub fn from_storage_json(raw: &str) -> Result<Self, SettingsError> {
        if raw.trim().is_empty() {
            return Ok(Self::new());
        }
        let mirror: BTreeMap<String, StoredValue<'static>> =
            serde_json::from_str(raw).map_err(|error| SettingsError::Corrupt(error.to_string()))?;
        Ok(Self(
            mirror
                .into_iter()
                .map(|(key, value)| (key, value.into_owned()))
                .collect(),
        ))
    }
}

/// The on-disk mirror of a [`SettingValue`], tagged so a secret survives a
/// round trip instead of collapsing into plain text.
///
/// Private, and the only shape that carries a credential's contents.
#[derive(Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
enum StoredValue<'a> {
    Secret { v: std::borrow::Cow<'a, str> },
    Toggle { v: bool },
    Number { v: i64 },
    Decimal { scale: u8, v: i64 },
    Text { v: std::borrow::Cow<'a, str> },
}

impl<'a> From<&'a SettingValue> for StoredValue<'a> {
    fn from(value: &'a SettingValue) -> Self {
        match value {
            SettingValue::Secret(secret) => Self::Secret {
                v: std::borrow::Cow::Borrowed(secret.expose()),
            },
            SettingValue::Toggle(v) => Self::Toggle { v: *v },
            SettingValue::Number(v) => Self::Number { v: *v },
            SettingValue::Decimal(scaled) => Self::Decimal {
                scale: scaled.scale,
                v: scaled.value,
            },
            SettingValue::Text(text) => Self::Text {
                v: std::borrow::Cow::Borrowed(text),
            },
        }
    }
}

impl StoredValue<'_> {
    fn into_owned(self) -> SettingValue {
        match self {
            Self::Secret { v } => SettingValue::Secret(SecretString::new(v.into_owned())),
            Self::Toggle { v } => SettingValue::Toggle(v),
            Self::Number { v } => SettingValue::Number(v),
            Self::Decimal { scale, v } => SettingValue::Decimal(Scaled::new(scale, v)),
            Self::Text { v } => SettingValue::Text(v.into_owned()),
        }
    }
}

/// What a form submits: raw JSON text per key, before any typing.
///
/// A browser form has strings in it, not `i64`s and `(scale, value)` pairs.
/// This is the shape that arrives, and [`SettingsSchema::validate`] is what
/// turns it into typed [`SettingValue`]s — server-side, always, whatever
/// the client already checked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettingsInput(BTreeMap<String, serde_json::Value>);

impl SettingsInput {
    /// An empty submission.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one raw value.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.0.insert(key.into(), value);
        self
    }

    /// The keys submitted.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Fills in every secret this submission left absent or blank from
    /// `previous`.
    ///
    /// A client renders its settings form from a document that, by design,
    /// never contained the credentials; submitting that form back must not
    /// therefore erase them. Applied before validation, so a *required*
    /// secret already on file satisfies the schema without being re-typed —
    /// which is why it belongs on the raw submission and not on the
    /// validated result.
    #[must_use]
    pub fn carry_secrets_from(
        mut self,
        previous: &SettingsValues,
        schema: &SettingsSchema,
    ) -> Self {
        for field in schema.fields.iter().filter(|f| f.is_secret()) {
            let supplied = self
                .0
                .get(&field.key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.is_empty());
            if supplied {
                continue;
            }
            if let Some(stored) = previous.secret(&field.key) {
                self.0.insert(
                    field.key.clone(),
                    serde_json::Value::String(stored.expose().to_owned()),
                );
            }
        }
        self
    }
}

impl SettingsSchema {
    /// A schema with these fields.
    #[must_use]
    pub fn new(fields: Vec<SettingField>) -> Self {
        Self { fields }
    }

    /// The field declared under `key`.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&SettingField> {
        self.fields.iter().find(|field| field.key == key)
    }

    /// Turns a form submission into typed values, applying defaults and
    /// rejecting everything that does not fit.
    ///
    /// Runs on the server on every write, never as a convenience the client
    /// could skip: a submission reaching this function may have been
    /// hand-written, and the client's own copy of the schema is only ever a
    /// courtesy to the person typing.
    ///
    /// # Errors
    /// [`SettingsError`] for an unknown key, a missing required value, a
    /// value of the wrong type, one outside the declared bounds, or a
    /// choice that is not an option.
    pub fn validate(&self, input: &SettingsInput) -> Result<SettingsValues, SettingsError> {
        for key in input.keys() {
            if self.field(key).is_none() {
                return Err(SettingsError::Unknown(key.to_owned()));
            }
        }

        let mut out = SettingsValues::new();
        for field in &self.fields {
            let raw = input.0.get(&field.key).filter(|v| !v.is_null());
            match Self::coerce(field, raw)? {
                Some(value) => out.set(field.key.clone(), value),
                None if field.required => return Err(SettingsError::Missing(field.key.clone())),
                None => {}
            }
        }
        Ok(out)
    }

    /// Re-validates already-typed values — the path an account's stored
    /// settings take when an adapter's schema has changed under them.
    ///
    /// # Errors
    /// As [`validate`](Self::validate).
    pub fn validate_values(
        &self,
        values: &SettingsValues,
    ) -> Result<SettingsValues, SettingsError> {
        let mut input = SettingsInput::new();
        for field in &self.fields {
            if let Some(value) = values.get(&field.key) {
                input = input.with(field.key.clone(), value_to_json(value));
            }
        }
        self.validate(&input)
    }

    fn coerce(
        field: &SettingField,
        raw: Option<&serde_json::Value>,
    ) -> Result<Option<SettingValue>, SettingsError> {
        match &field.kind {
            FieldKind::Secret { .. } => Ok(raw
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| SettingValue::Secret(SecretString::new(text)))),
            FieldKind::Toggle { default } => {
                let value = match raw {
                    Some(value) => value.as_bool().ok_or_else(|| SettingsError::WrongType {
                        field: field.key.clone(),
                        expected: "true or false".to_owned(),
                    })?,
                    None => *default,
                };
                Ok(Some(SettingValue::Toggle(value)))
            }
            FieldKind::Choice { default, options } => {
                let picked = match raw {
                    Some(value) => value
                        .as_str()
                        .ok_or_else(|| SettingsError::WrongType {
                            field: field.key.clone(),
                            expected: "one of the listed options".to_owned(),
                        })?
                        .to_owned(),
                    None => match default {
                        Some(value) => value.clone(),
                        None => return Ok(None),
                    },
                };
                if !options.iter().any(|option| option.value == picked) {
                    return Err(SettingsError::NotAnOption {
                        field: field.key.clone(),
                        options: options
                            .iter()
                            .map(|option| option.value.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
                Ok(Some(SettingValue::Text(picked)))
            }
            _ => Self::coerce_numeric(field, raw),
        }
    }

    /// The kinds carrying bounds to check: text length, whole numbers and
    /// fixed-point decimals.
    fn coerce_numeric(
        field: &SettingField,
        raw: Option<&serde_json::Value>,
    ) -> Result<Option<SettingValue>, SettingsError> {
        let key = || field.key.clone();
        match &field.kind {
            // Handled by `coerce` before this is reached.
            FieldKind::Secret { .. } | FieldKind::Toggle { .. } | FieldKind::Choice { .. } => {
                Ok(None)
            }
            FieldKind::Text {
                default, max_len, ..
            } => {
                let text = match raw {
                    Some(value) => value
                        .as_str()
                        .ok_or_else(|| SettingsError::WrongType {
                            field: key(),
                            expected: "text".to_owned(),
                        })?
                        .to_owned(),
                    None => match default {
                        Some(value) => value.clone(),
                        None => return Ok(None),
                    },
                };
                if text.chars().count() > *max_len {
                    return Err(SettingsError::TooLong {
                        field: key(),
                        max: *max_len,
                    });
                }
                Ok(Some(SettingValue::Text(text)))
            }
            FieldKind::Number {
                default, min, max, ..
            } => {
                let value = match raw {
                    Some(value) => {
                        number_from_json(value).ok_or_else(|| SettingsError::WrongType {
                            field: key(),
                            expected: "a whole number".to_owned(),
                        })?
                    }
                    None => match default {
                        Some(value) => *value,
                        None => return Ok(None),
                    },
                };
                if value < *min || value > *max {
                    return Err(SettingsError::OutOfRange {
                        field: key(),
                        min: min.to_string(),
                        max: max.to_string(),
                    });
                }
                Ok(Some(SettingValue::Number(value)))
            }
            FieldKind::Decimal {
                scale,
                default,
                min,
                max,
                ..
            } => {
                let value = match raw {
                    Some(value) => decimal_from_json(value, *scale).ok_or_else(|| {
                        SettingsError::WrongType {
                            field: key(),
                            expected: format!("a decimal with at most {scale} decimal places"),
                        }
                    })?,
                    None => match default {
                        Some(value) => *value,
                        None => return Ok(None),
                    },
                };
                if value < *min || value > *max {
                    return Err(SettingsError::OutOfRange {
                        field: key(),
                        min: format_scaled(*min, *scale),
                        max: format_scaled(*max, *scale),
                    });
                }
                Ok(Some(SettingValue::Decimal(Scaled::new(*scale, value))))
            }
        }
    }
}

/// A whole number from JSON, accepting the string form a form control
/// produces as readily as a bare number — but never a fractional one, which
/// would have to be rounded to fit.
fn number_from_json(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A fixed-point decimal from JSON at exactly `scale`.
///
/// A JSON number is rendered through its own `Display` first rather than
/// read as an `f64`: `0.1` has no exact binary form, and this project's
/// rule is that a value which ends up deciding money never routes through
/// floating point. `serde_json` keeps the literal text of a number, so the
/// digits a user typed are the digits that get parsed.
fn decimal_from_json(value: &serde_json::Value, scale: u8) -> Option<i64> {
    match value {
        serde_json::Value::String(text) => parse_scaled(text, scale),
        serde_json::Value::Number(number) => parse_scaled(&number.to_string(), scale),
        _ => None,
    }
}

/// The JSON a typed value came from, for re-validation.
fn value_to_json(value: &SettingValue) -> serde_json::Value {
    match value {
        // The credential's text, in memory only: `SettingsInput` is never
        // serialised outward — it is the shape a request arrives in, not
        // one a response is built from — so this does not reopen the path
        // `SecretString`'s own `Serialize` impl closes.
        SettingValue::Secret(secret) => serde_json::Value::String(secret.expose().to_owned()),
        SettingValue::Toggle(v) => serde_json::Value::Bool(*v),
        SettingValue::Number(v) => serde_json::Value::Number((*v).into()),
        SettingValue::Decimal(scaled) => {
            serde_json::Value::String(format_scaled(scaled.value, scaled.scale))
        }
        SettingValue::Text(text) => serde_json::Value::String(text.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChoiceOption, FieldKind, SecretString, SettingField, SettingValue, SettingsError,
        SettingsInput, SettingsSchema, SettingsValues,
    };
    use senken_core::decimal::Scaled;
    use serde_json::json;

    fn schema() -> SettingsSchema {
        SettingsSchema::new(vec![
            SettingField::new(
                "api_key",
                "API key",
                FieldKind::Secret {
                    placeholder: String::new(),
                },
            ),
            SettingField::new(
                "starting_balance",
                "Starting balance",
                FieldKind::Decimal {
                    scale: 2,
                    default: Some(10_000_000),
                    min: 0,
                    max: 100_000_000_000,
                    unit: "USD".to_owned(),
                },
            ),
            SettingField::new(
                "leverage",
                "Leverage",
                FieldKind::Number {
                    default: Some(1),
                    min: 1,
                    max: 125,
                    unit: "x".to_owned(),
                },
            ),
            SettingField::new(
                "mode",
                "Position mode",
                FieldKind::Choice {
                    default: Some("netting".to_owned()),
                    options: vec![
                        ChoiceOption::new("netting", "Netting"),
                        ChoiceOption::new("hedging", "Hedging"),
                    ],
                },
            ),
            SettingField::new(
                "allow_short",
                "Allow shorting",
                FieldKind::Toggle { default: true },
            ),
        ])
    }

    #[test]
    fn a_secret_serialises_as_null_and_never_as_its_contents() {
        let secret = SecretString::new("sk-live-abc123");
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, "null");
        assert!(
            !json.contains("abc123"),
            "a credential must have no path out through Serialize at all"
        );
    }

    #[test]
    fn a_secret_is_redacted_in_debug_output_too() {
        // A tracing field or a panic message is as much a leak as a
        // response body.
        let rendered = format!("{:?}", SecretString::new("sk-live-abc123"));
        assert!(!rendered.contains("abc123"), "got {rendered}");
        assert_eq!(rendered, "SecretString(set)");
    }

    #[test]
    fn settings_carrying_a_secret_serialise_it_as_null_through_the_whole_map() {
        // The guarantee has to survive being nested inside the value the
        // API actually returns, not just a bare `SecretString`.
        let mut values = SettingsValues::new();
        values.set(
            "api_key",
            SettingValue::Secret(SecretString::new("sk-live-abc123")),
        );
        values.set("leverage", SettingValue::Number(10));

        let json = serde_json::to_string(&values).unwrap();

        assert!(!json.contains("abc123"), "got {json}");
        assert_eq!(json, r#"{"api_key":null,"leverage":10}"#);
    }

    #[test]
    fn storage_json_is_the_one_path_that_does_write_a_secret() {
        let mut values = SettingsValues::new();
        values.set(
            "api_key",
            SettingValue::Secret(SecretString::new("sk-live-abc123")),
        );

        let stored = values.to_storage_json().unwrap();

        assert!(
            stored.contains("abc123"),
            "the store must be able to persist it"
        );
        let read_back = SettingsValues::from_storage_json(&stored).unwrap();
        assert_eq!(
            read_back.secret("api_key").unwrap().expose(),
            "sk-live-abc123"
        );
    }

    #[test]
    fn every_value_kind_survives_a_storage_round_trip_with_its_type_intact() {
        // An untagged `SettingValue` would read `true` back as text or a
        // decimal back as a plain number; the tagged storage mirror is what
        // stops that.
        let mut values = SettingsValues::new();
        values.set("api_key", SettingValue::Secret(SecretString::new("s")));
        values.set("allow_short", SettingValue::Toggle(false));
        values.set("leverage", SettingValue::Number(10));
        values.set(
            "starting_balance",
            SettingValue::Decimal(Scaled::new(2, 25_000)),
        );
        values.set("mode", SettingValue::Text("hedging".to_owned()));

        let read_back =
            SettingsValues::from_storage_json(&values.to_storage_json().unwrap()).unwrap();

        assert_eq!(read_back, values);
        assert_eq!(read_back.decimal("starting_balance").unwrap().scale, 2);
    }

    #[test]
    fn validation_applies_defaults_for_absent_fields() {
        let values = schema()
            .validate(&SettingsInput::new().with("api_key", json!("sk-1")))
            .unwrap();

        assert_eq!(
            values.decimal("starting_balance"),
            Some(Scaled::new(2, 10_000_000))
        );
        assert_eq!(values.number("leverage"), Some(1));
        assert_eq!(values.text("mode"), Some("netting"));
        assert_eq!(values.toggle("allow_short"), Some(true));
    }

    #[test]
    fn a_decimal_is_parsed_from_its_digits_and_never_through_a_float() {
        // 0.1 has no exact binary form; parsing the literal text is what
        // keeps a money value exact.
        let values = schema()
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("sk-1"))
                    .with("starting_balance", json!("0.10")),
            )
            .unwrap();

        assert_eq!(values.decimal("starting_balance"), Some(Scaled::new(2, 10)));
    }

    #[test]
    fn a_decimal_finer_than_the_field_allows_is_rejected_not_rounded() {
        let error = schema()
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("sk-1"))
                    .with("starting_balance", json!("1.005")),
            )
            .unwrap_err();

        assert!(
            matches!(error, SettingsError::WrongType { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_value_outside_the_declared_bounds_is_rejected() {
        let error = schema()
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("sk-1"))
                    .with("leverage", json!(500)),
            )
            .unwrap_err();

        assert_eq!(
            error,
            SettingsError::OutOfRange {
                field: "leverage".to_owned(),
                min: "1".to_owned(),
                max: "125".to_owned(),
            }
        );
    }

    #[test]
    fn a_choice_outside_its_options_is_rejected() {
        let error = schema()
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("sk-1"))
                    .with("mode", json!("both")),
            )
            .unwrap_err();

        assert!(
            matches!(error, SettingsError::NotAnOption { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_key_the_schema_does_not_declare_is_rejected_rather_than_ignored() {
        // Silently dropping a setting a user typed is indistinguishable
        // from the setting having no effect.
        let error = schema()
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("sk-1"))
                    .with("secret_backdoor", json!("1")),
            )
            .unwrap_err();

        assert_eq!(error, SettingsError::Unknown("secret_backdoor".to_owned()));
    }

    #[test]
    fn a_required_field_with_no_default_and_no_value_fails() {
        let error = schema().validate(&SettingsInput::new()).unwrap_err();
        assert_eq!(error, SettingsError::Missing("api_key".to_owned()));
    }

    #[test]
    fn resubmitting_a_form_that_never_saw_the_secret_keeps_the_stored_one() {
        // The exact round trip a settings dialog performs: it renders from
        // a document with `api_key: null` in it and posts that back.
        let schema = schema();
        let stored = schema
            .validate(&SettingsInput::new().with("api_key", json!("sk-live-abc123")))
            .unwrap();

        let resubmitted = schema
            .validate(
                &SettingsInput::new()
                    .with("leverage", json!(20))
                    .carry_secrets_from(&stored, &schema),
            )
            .unwrap();

        assert_eq!(
            resubmitted.secret("api_key").map(SecretString::expose),
            Some("sk-live-abc123"),
            "an edit that did not re-type the credential must not erase it"
        );
    }

    #[test]
    fn carrying_secrets_forward_does_not_overwrite_a_freshly_typed_one() {
        let schema = schema();
        let stored = schema
            .validate(&SettingsInput::new().with("api_key", json!("old")))
            .unwrap();
        let submitted = schema
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("new"))
                    .carry_secrets_from(&stored, &schema),
            )
            .unwrap();

        assert_eq!(submitted.secret("api_key").unwrap().expose(), "new");
    }

    #[test]
    fn secret_status_reports_only_whether_a_credential_exists() {
        let schema = schema();
        let values = schema
            .validate(&SettingsInput::new().with("api_key", json!("sk-1")))
            .unwrap();

        let status = values.secret_status(&schema);

        assert_eq!(status.get("api_key"), Some(&true));
        assert_eq!(status.len(), 1, "only secret fields belong in this map");
    }

    #[test]
    fn revalidating_stored_values_keeps_the_secret_and_the_types() {
        let schema = schema();
        let stored = schema
            .validate(
                &SettingsInput::new()
                    .with("api_key", json!("sk-1"))
                    .with("starting_balance", json!("250.00")),
            )
            .unwrap();

        let revalidated = schema.validate_values(&stored).unwrap();

        assert_eq!(revalidated, stored);
        assert_eq!(revalidated.secret("api_key").unwrap().expose(), "sk-1");
    }

    #[test]
    fn revalidating_fails_when_stored_values_no_longer_fit_a_changed_schema() {
        let old = SettingsSchema::new(vec![SettingField::new(
            "leverage",
            "Leverage",
            FieldKind::Number {
                default: None,
                min: 1,
                max: 500,
                unit: String::new(),
            },
        )]);
        let stored = old
            .validate(&SettingsInput::new().with("leverage", json!(400)))
            .unwrap();

        let tightened = SettingsSchema::new(vec![SettingField::new(
            "leverage",
            "Leverage",
            FieldKind::Number {
                default: None,
                min: 1,
                max: 125,
                unit: String::new(),
            },
        )]);

        assert!(tightened.validate_values(&stored).is_err());
    }

    #[test]
    fn empty_stored_text_reads_back_as_no_settings() {
        assert!(SettingsValues::from_storage_json("").unwrap().is_empty());
    }
}
