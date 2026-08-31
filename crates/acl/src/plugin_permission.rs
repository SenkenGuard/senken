//! Plugin-declared permissions: a namespace a plugin owns, and the
//! permissions it names inside that namespace.
//!
//! `Resource` (see [`crate::Resource`]) is a closed enum by design — that is
//! exactly what makes [`crate::decide`]'s exhaustive match possible. A
//! plugin's permissions are the opposite: open-ended, named at the plugin's
//! own discretion, sometimes only known at runtime (a plugin mirroring an
//! external system's resources cannot know them at build time). Folding
//! them into `Resource` would either close off that flexibility or reopen
//! the exhaustiveness hole B7 exists to close, so they are modelled as a
//! separate, string-identified permission instead: [`PluginPermissionName`].
//!
//! **A plugin may register a permission; it may never grant one — not even
//! to itself.** This module only ever hands a plugin a [`PluginNamespace`],
//! whose sole capability is [`declare`](PluginNamespace::declare) /
//! [`admit`](PluginNamespace::admit): naming a permission so an admin can
//! later assign it to a role. Neither `PluginNamespace` nor
//! [`PluginPermissionName`] has any method that returns a
//! [`crate::Grant`], borrows a [`crate::Role`] or [`crate::Actor`], or
//! otherwise reaches [`crate::decide`] — attaching a plugin permission to a
//! role is a storage/admin operation with no representation in this crate
//! at all, exactly so a plugin cannot reach it by construction.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Separates a permission's namespace from its `resource:operation` pair:
/// `<plugin-id>.<resource>:<operation>`.
pub const NAMESPACE_SEPARATOR: char = '.';
/// Separates a permission's resource from its operation, after the
/// namespace: `<plugin-id>.<resource>:<operation>`.
pub const OPERATION_SEPARATOR: char = ':';

/// Why a string is not a valid [`PluginPermissionName`], or why a namespace
/// refused to admit one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PluginPermissionError {
    /// Nothing but whitespace.
    #[error("plugin permission name is empty")]
    Empty,

    /// No `.` anywhere in the input.
    #[error(
        "plugin permission `{0}` is missing the `.` separating the plugin namespace from the rest"
    )]
    MissingNamespaceSeparator(String),

    /// No `:` after the namespace separator.
    #[error(
        "plugin permission `{0}` is missing the `:` separating its resource from its operation"
    )]
    MissingOperationSeparator(String),

    /// Nothing before the `.`.
    #[error("plugin permission `{0}` has an empty namespace")]
    EmptyNamespace(String),

    /// Nothing between the `.` and the `:`.
    #[error("plugin permission `{0}` has an empty resource")]
    EmptyResource(String),

    /// Nothing after the `:`.
    #[error("plugin permission `{0}` has an empty operation")]
    EmptyOperation(String),

    /// A segment (namespace, resource or operation) is not lowercase
    /// `[a-z0-9-]`.
    #[error("plugin permission segment `{0}` must be lowercase [a-z0-9-]")]
    InvalidSegment(String),

    /// A namespace tried to admit a permission naming a different
    /// namespace — the manifest delegates authority over its own subtree
    /// only, like a DNS zone, and cannot register into another plugin's
    /// (or core's) namespace.
    #[error("permission `{permission}` is outside namespace `{namespace}`")]
    OutsideNamespace {
        /// The namespace that refused the permission.
        namespace: String,
        /// The permission it was asked to admit.
        permission: String,
    },
}

/// A plugin-declared permission name: `<plugin-id>.<resource>:<operation>`, e.g. `mychart.dashboard:view`.
///
/// Namespacing is a security boundary, not tidiness: without the
/// plugin-id prefix, two plugins both declaring `chart:view` would collide,
/// and an admin granting one would silently grant the other. Stored as a
/// single allocation plus two offsets, mirroring
/// `senken_marketdata::InstrumentId`'s `source:symbol` encoding.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginPermissionName {
    raw: Box<str>,
    namespace_end: u16,
    resource_end: u16,
}

impl PluginPermissionName {
    /// Parses `<plugin-id>.<resource>:<operation>`, trimming surrounding
    /// whitespace.
    ///
    /// # Errors
    /// See [`PluginPermissionError`].
    ///
    /// # Examples
    /// ```
    /// use senken_acl::PluginPermissionName;
    ///
    /// let name = PluginPermissionName::parse("mychart.dashboard:view")?;
    /// assert_eq!(name.namespace(), "mychart");
    /// assert_eq!(name.resource(), "dashboard");
    /// assert_eq!(name.operation(), "view");
    /// # Ok::<(), senken_acl::PluginPermissionError>(())
    /// ```
    pub fn parse(raw: &str) -> Result<Self, PluginPermissionError> {
        let raw = raw.trim();
        let (namespace_end, resource_end) = Self::validate(raw)?;
        Ok(Self {
            raw: raw.into(),
            namespace_end,
            resource_end,
        })
    }

    /// Checks a trimmed permission string and returns the offsets of its
    /// two separators.
    fn validate(raw: &str) -> Result<(u16, u16), PluginPermissionError> {
        if raw.is_empty() {
            return Err(PluginPermissionError::Empty);
        }

        let Some(namespace_end) = raw.find(NAMESPACE_SEPARATOR) else {
            return Err(PluginPermissionError::MissingNamespaceSeparator(
                raw.to_owned(),
            ));
        };
        let rest = &raw[namespace_end + 1..];
        let Some(operation_offset) = rest.find(OPERATION_SEPARATOR) else {
            return Err(PluginPermissionError::MissingOperationSeparator(
                raw.to_owned(),
            ));
        };
        let resource_end = namespace_end + 1 + operation_offset;

        let namespace = &raw[..namespace_end];
        let resource = &raw[namespace_end + 1..resource_end];
        let operation = &raw[resource_end + 1..];

        if namespace.is_empty() {
            return Err(PluginPermissionError::EmptyNamespace(raw.to_owned()));
        }
        if resource.is_empty() {
            return Err(PluginPermissionError::EmptyResource(raw.to_owned()));
        }
        if operation.is_empty() {
            return Err(PluginPermissionError::EmptyOperation(raw.to_owned()));
        }
        for segment in [namespace, resource, operation] {
            if !is_valid_slug(segment) {
                return Err(PluginPermissionError::InvalidSegment(segment.to_owned()));
            }
        }

        // Both offsets are found within `raw`, whose length a permission
        // name has no reason to grow past `u16::MAX`; kept honest anyway
        // rather than truncating silently.
        let namespace_end =
            u16::try_from(namespace_end).map_err(|_| PluginPermissionError::Empty)?;
        let resource_end = u16::try_from(resource_end).map_err(|_| PluginPermissionError::Empty)?;
        Ok((namespace_end, resource_end))
    }

    /// The plugin namespace this permission belongs to, e.g. `mychart`.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.raw[..usize::from(self.namespace_end)]
    }

    /// The resource half, e.g. `dashboard`.
    #[must_use]
    pub fn resource(&self) -> &str {
        &self.raw[usize::from(self.namespace_end) + 1..usize::from(self.resource_end)]
    }

    /// The operation half, e.g. `view`.
    #[must_use]
    pub fn operation(&self) -> &str {
        &self.raw[usize::from(self.resource_end) + 1..]
    }

    /// The whole name, `<plugin-id>.<resource>:<operation>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

/// `true` when `segment` is non-empty, lowercase ASCII letters, digits and
/// hyphens only — the same rule `senken_marketdata::InstrumentId` applies
/// to its source id.
fn is_valid_slug(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl fmt::Display for PluginPermissionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.raw)
    }
}

impl fmt::Debug for PluginPermissionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PluginPermissionName({})", self.raw)
    }
}

impl std::str::FromStr for PluginPermissionName {
    type Err = PluginPermissionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for PluginPermissionName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for PluginPermissionName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;

        impl Visitor<'_> for NameVisitor {
            type Value = PluginPermissionName;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a plugin permission of the form `<plugin-id>.<resource>:<operation>`")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                PluginPermissionName::parse(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(NameVisitor)
    }
}

/// A plugin's authority over one namespace of permission names.
///
/// The manifest delegates authority over this subtree, like a DNS zone: a
/// plugin registered as `mychart` may name permissions under `mychart.*`
/// and nothing else — it cannot register `senken.users:manage`, whether by
/// mistake or by malice. Obtaining a `PluginNamespace` for a plugin id is
/// the *only* authority this crate models; there is no corresponding type
/// for "may grant", because that authority does not exist for plugins at
/// all (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginNamespace(Box<str>);

impl PluginNamespace {
    /// A namespace for the plugin id `id` (the same id as
    /// `senken_plugin::PluginManifest::id`, e.g. `mychart`).
    ///
    /// # Errors
    /// [`PluginPermissionError::EmptyNamespace`] for an empty id,
    /// [`PluginPermissionError::InvalidSegment`] for one that is not
    /// lowercase `[a-z0-9-]`.
    pub fn new(id: &str) -> Result<Self, PluginPermissionError> {
        let id = id.trim();
        if id.is_empty() {
            return Err(PluginPermissionError::EmptyNamespace(String::new()));
        }
        if !is_valid_slug(id) {
            return Err(PluginPermissionError::InvalidSegment(id.to_owned()));
        }
        Ok(Self(id.into()))
    }

    /// The plugin id this namespace was constructed for.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.0
    }

    /// Names a new permission, `<self>.<resource>:<operation>`, within this
    /// namespace.
    ///
    /// This is the only way this crate offers a plugin to bring a
    /// permission name into existence — it cannot fail on the namespace
    /// axis, because the namespace is `self` by construction. It can still
    /// fail if `resource` or `operation` is not a valid slug.
    ///
    /// # Errors
    /// See [`PluginPermissionError`].
    pub fn declare(
        &self,
        resource: &str,
        operation: &str,
    ) -> Result<PluginPermissionName, PluginPermissionError> {
        let candidate = format!("{}.{resource}:{operation}", self.0);
        PluginPermissionName::parse(&candidate)
    }

    /// Admits an already-parsed permission name, iff it names this
    /// namespace.
    ///
    /// This is the defensive twin of [`declare`](Self::declare), for a
    /// fully-qualified name that arrived from elsewhere (a manifest field,
    /// an admin request) rather than being built by this namespace itself
    ///   — the case B9 means by "it cannot register `senken.users:manage`".
    ///
    /// # Errors
    /// [`PluginPermissionError::OutsideNamespace`] when `permission`'s
    /// namespace is not `self`.
    pub fn admit(
        &self,
        permission: PluginPermissionName,
    ) -> Result<PluginPermissionName, PluginPermissionError> {
        if permission.namespace() == self.id() {
            Ok(permission)
        } else {
            Err(PluginPermissionError::OutsideNamespace {
                namespace: self.id().to_owned(),
                permission: permission.as_str().to_owned(),
            })
        }
    }
}

/// Whether a registered permission is still declared by the plugin that
/// named it (mirroring the `orphaned` column of the `plugin_permissions` table —).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum PluginPermissionState {
    /// The owning plugin currently declares this permission.
    Registered,
    /// The owning plugin stopped declaring this permission — uninstalled,
    /// updated, or its manifest changed — while a role still references
    /// it. The permission is *not* dropped from that role: silently
    /// removing it would silently narrow every role that held it, with no
    /// record of why. An admin decides what to do with an orphan.
    Orphaned,
}

/// A permission a plugin has registered, and whether it is still declared.
///
/// This is the pure-data shape a storage layer persists as one row of
/// `plugin_permissions`; it carries no timestamp because
/// producing one needs a clock, which is an I/O concern this crate does
/// not have.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PluginPermissionRecord {
    name: PluginPermissionName,
    state: PluginPermissionState,
}

impl PluginPermissionRecord {
    /// A freshly registered permission.
    #[must_use]
    pub fn registered(name: PluginPermissionName) -> Self {
        Self {
            name,
            state: PluginPermissionState::Registered,
        }
    }

    /// The permission name.
    #[must_use]
    pub fn name(&self) -> &PluginPermissionName {
        &self.name
    }

    /// The current registration state.
    #[must_use]
    pub fn state(&self) -> PluginPermissionState {
        self.state
    }

    /// `true` when the owning plugin no longer declares this permission.
    #[must_use]
    pub fn is_orphaned(&self) -> bool {
        matches!(self.state, PluginPermissionState::Orphaned)
    }

    /// Marks the permission orphaned because its plugin stopped declaring
    /// it.
    #[must_use]
    pub fn orphan(mut self) -> Self {
        self.state = PluginPermissionState::Orphaned;
        self
    }

    /// Marks a previously orphaned permission registered again, because its
    /// plugin has resumed declaring it.
    #[must_use]
    pub fn re_register(mut self) -> Self {
        self.state = PluginPermissionState::Registered;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PluginNamespace, PluginPermissionError, PluginPermissionName, PluginPermissionRecord,
    };

    #[test]
    fn parse_splits_namespace_resource_and_operation() {
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        assert_eq!(name.namespace(), "mychart");
        assert_eq!(name.resource(), "dashboard");
        assert_eq!(name.operation(), "view");
        assert_eq!(name.as_str(), "mychart.dashboard:view");
    }

    #[test]
    fn parse_rejects_a_missing_namespace_separator() {
        assert_eq!(
            PluginPermissionName::parse("dashboard:view"),
            Err(PluginPermissionError::MissingNamespaceSeparator(
                "dashboard:view".to_owned()
            ))
        );
    }

    #[test]
    fn parse_rejects_a_missing_operation_separator() {
        assert_eq!(
            PluginPermissionName::parse("mychart.dashboard"),
            Err(PluginPermissionError::MissingOperationSeparator(
                "mychart.dashboard".to_owned()
            ))
        );
    }

    #[test]
    fn parse_rejects_an_empty_segment() {
        assert!(PluginPermissionName::parse(".dashboard:view").is_err());
        assert!(PluginPermissionName::parse("mychart.:view").is_err());
        assert!(PluginPermissionName::parse("mychart.dashboard:").is_err());
    }

    #[test]
    fn parse_rejects_uppercase_or_symbols_in_any_segment() {
        assert!(PluginPermissionName::parse("MyChart.dashboard:view").is_err());
        assert!(PluginPermissionName::parse("mychart.dash board:view").is_err());
    }

    #[test]
    fn parse_rejects_a_second_colon() {
        // The operation segment is a slug like any other, so it cannot
        // itself contain `:` — a second colon does not get folded into the
        // operation, it makes the operation segment invalid.
        assert_eq!(
            PluginPermissionName::parse("mychart.dashboard:view:detailed"),
            Err(PluginPermissionError::InvalidSegment(
                "view:detailed".to_owned()
            ))
        );
    }

    #[test]
    fn namespace_splits_on_the_first_dot_even_if_more_dots_would_be_invalid_anyway() {
        // Namespace is a slug too, so it cannot contain `.` — parsing splits
        // on the *first* `.` regardless, which is what makes the split
        // unambiguous rather than a lucky consequence of the slug rule.
        assert!(PluginPermissionName::parse("my.chart.dashboard:view").is_err());
    }

    #[test]
    fn a_namespace_declares_permissions_under_itself() {
        let namespace = PluginNamespace::new("mychart").unwrap();
        let name = namespace.declare("dashboard", "view").unwrap();
        assert_eq!(name.as_str(), "mychart.dashboard:view");
    }

    #[test]
    fn a_namespace_admits_a_permission_naming_itself() {
        let namespace = PluginNamespace::new("mychart").unwrap();
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        assert!(namespace.admit(name).is_ok());
    }

    #[test]
    fn a_namespace_refuses_to_admit_a_permission_naming_another_namespace() {
        // The scenario B9 calls out by name: a plugin manifest delegates
        // authority over its own subtree only, and cannot register into
        // core's `senken` namespace (or any other plugin's).
        let namespace = PluginNamespace::new("mychart").unwrap();
        let foreign = PluginPermissionName::parse("senken.users:manage").unwrap();

        let err = namespace.admit(foreign).unwrap_err();
        assert_eq!(
            err,
            PluginPermissionError::OutsideNamespace {
                namespace: "mychart".to_owned(),
                permission: "senken.users:manage".to_owned(),
            }
        );
    }

    #[test]
    fn a_freshly_registered_permission_is_not_orphaned() {
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        let record = PluginPermissionRecord::registered(name);
        assert!(!record.is_orphaned());
    }

    #[test]
    fn orphan_marks_a_registered_permission_orphaned() {
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        let record = PluginPermissionRecord::registered(name).orphan();
        assert!(record.is_orphaned());
    }

    #[test]
    fn re_register_clears_the_orphaned_state() {
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        let record = PluginPermissionRecord::registered(name)
            .orphan()
            .re_register();
        assert!(!record.is_orphaned());
    }
}
