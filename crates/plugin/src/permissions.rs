//! Reconciling a plugin's previously known permissions against what it
//! declares this activation (the orphan rule).
//!
//! Nothing here touches storage: the caller loads `previous` from wherever
//! `plugin_permissions` ends up persisted and writes back the
//! result — this crate has no database dependency to do either itself.

use std::collections::{BTreeMap, BTreeSet};

use senken_acl::{PluginPermissionName, PluginPermissionRecord};

/// Reconciles `previous` (the last known state of every permission this
/// plugin has ever registered, `Registered` or `Orphaned`) against
/// `current` (every name it declares this activation — its manifest's
/// static [`PluginManifest::permissions`](crate::PluginManifest::permissions)
/// plus anything registered at runtime through
/// [`ActivationContext::register_plugin_permission`](crate::ActivationContext::register_plugin_permission)).
///
/// - A name in `current` with no `previous` record is newly
///   [`PluginPermissionRecord::registered`].
/// - A name `Registered` in `previous` but absent from `current` is
///   [`orphan`](PluginPermissionRecord::orphan)ed, not dropped — a role that
///   still references it must look broken, not quietly shrink, so an admin can see the access changed instead of it silently
///   vanishing.
/// - A name `Orphaned` in `previous` that reappears in `current` is
///   [`re_register`](PluginPermissionRecord::re_register)ed.
/// - A name `Orphaned` in `previous` and still absent from `current` stays
///   orphaned.
///
/// # Examples
/// ```
/// use senken_acl::{PluginPermissionName, PluginPermissionRecord};
/// use senken_plugin::reconcile_plugin_permissions;
///
/// let gone = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
/// let previous = vec![PluginPermissionRecord::registered(gone.clone())];
///
/// // The plugin no longer declares `gone` this activation.
/// let reconciled = reconcile_plugin_permissions(&previous, &[]);
///
/// assert_eq!(reconciled.len(), 1);
/// assert!(reconciled[0].is_orphaned());
/// ```
#[must_use]
pub fn reconcile_plugin_permissions(
    previous: &[PluginPermissionRecord],
    current: &[PluginPermissionName],
) -> Vec<PluginPermissionRecord> {
    let current: BTreeSet<&PluginPermissionName> = current.iter().collect();

    // Every previously known permission: orphaned if no longer declared,
    // re-registered if it reappeared, left alone otherwise.
    let mut reconciled: BTreeMap<PluginPermissionName, PluginPermissionRecord> = previous
        .iter()
        .cloned()
        .map(|record| {
            let name = record.name().clone();
            let declared = current.contains(&name);
            let record = match (record.is_orphaned(), declared) {
                (false, false) => record.orphan(),
                (true, true) => record.re_register(),
                _ => record,
            };
            (name, record)
        })
        .collect();

    // Anything declared this activation with no prior record at all is
    // freshly registered.
    for name in current {
        reconciled
            .entry(name.clone())
            .or_insert_with(|| PluginPermissionRecord::registered(name.clone()));
    }

    reconciled.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::reconcile_plugin_permissions;
    use senken_acl::{PluginPermissionName, PluginPermissionRecord};

    fn name(s: &str) -> PluginPermissionName {
        PluginPermissionName::parse(s).unwrap()
    }

    #[test]
    fn a_permission_declared_with_no_prior_record_is_newly_registered() {
        let view = name("mychart.dashboard:view");
        let reconciled = reconcile_plugin_permissions(&[], std::slice::from_ref(&view));

        assert_eq!(reconciled, vec![PluginPermissionRecord::registered(view)]);
    }

    #[test]
    fn a_registered_permission_that_stops_being_declared_is_orphaned_not_dropped() {
        let view = name("mychart.dashboard:view");
        let previous = vec![PluginPermissionRecord::registered(view.clone())];

        let reconciled = reconcile_plugin_permissions(&previous, &[]);

        assert_eq!(
            reconciled,
            vec![PluginPermissionRecord::registered(view).orphan()],
            "the permission must still be present, just marked orphaned"
        );
    }

    #[test]
    fn an_orphaned_permission_that_reappears_is_re_registered() {
        let view = name("mychart.dashboard:view");
        let previous = vec![PluginPermissionRecord::registered(view.clone()).orphan()];

        let reconciled = reconcile_plugin_permissions(&previous, std::slice::from_ref(&view));

        assert_eq!(reconciled, vec![PluginPermissionRecord::registered(view)]);
    }

    #[test]
    fn an_orphaned_permission_that_is_still_absent_stays_orphaned() {
        let view = name("mychart.dashboard:view");
        let previous = vec![PluginPermissionRecord::registered(view.clone()).orphan()];

        let reconciled = reconcile_plugin_permissions(&previous, &[]);

        assert_eq!(
            reconciled,
            vec![PluginPermissionRecord::registered(view).orphan()]
        );
    }

    #[test]
    fn a_still_declared_registered_permission_is_left_registered() {
        let view = name("mychart.dashboard:view");
        let previous = vec![PluginPermissionRecord::registered(view.clone())];

        let reconciled = reconcile_plugin_permissions(&previous, std::slice::from_ref(&view));

        assert_eq!(reconciled, vec![PluginPermissionRecord::registered(view)]);
    }

    #[test]
    fn unrelated_permissions_are_reconciled_independently() {
        let view = name("mychart.dashboard:view");
        let edit = name("mychart.dashboard:edit");
        // `view` is being dropped this activation; `edit` is brand new.
        let previous = vec![PluginPermissionRecord::registered(view.clone())];

        let mut reconciled = reconcile_plugin_permissions(&previous, std::slice::from_ref(&edit));
        reconciled.sort_by(|a, b| a.name().cmp(b.name()));

        assert_eq!(
            reconciled,
            vec![
                PluginPermissionRecord::registered(edit),
                PluginPermissionRecord::registered(view).orphan(),
            ]
        );
    }
}
