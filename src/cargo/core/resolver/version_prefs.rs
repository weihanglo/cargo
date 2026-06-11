//! This module implements support for preferring some versions of a package
//! over other versions.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use cargo_util_schemas::core::PartialVersion;

use crate::core::Dependency;
use crate::core::PackageId;
use crate::core::SourceId;
use crate::core::Summary;
use crate::util::CargoResult;
use crate::util::GlobalContext;
use crate::util::context::CargoResolverConfig;
use crate::util::context::IncompatiblePublishAge;
use crate::util::interning::InternedString;
use crate::util::time_span::maybe_parse_time_span;

/// A collection of preferences for particular package versions.
///
/// This is built up with [`Self::prefer_package_id`] and [`Self::prefer_dependency`], then used to sort the set of
/// summaries for a package during resolution via [`Self::sort_summaries`].
///
/// As written, a version is either "preferred" or "not preferred".  Later extensions may
/// introduce more granular preferences.
#[derive(Default)]
pub struct VersionPreferences {
    try_to_use: HashSet<PackageId>,
    prefer_patch_deps: HashMap<InternedString, HashSet<Dependency>>,
    version_ordering: VersionOrdering,
    rust_versions: Vec<PartialVersion>,
    publish_time: Option<jiff::Timestamp>,
    publish_age: Option<PublishAgePolicy>,
}

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Debug)]
pub enum VersionOrdering {
    #[default]
    MaximumVersionsFirst,
    MinimumVersionsFirst,
}

impl VersionPreferences {
    /// Indicate that the given package (specified as a [`PackageId`]) should be preferred.
    pub fn prefer_package_id(&mut self, pkg_id: PackageId) {
        self.try_to_use.insert(pkg_id);
    }

    /// Indicate that the given package (specified as a [`Dependency`])  should be preferred.
    pub fn prefer_dependency(&mut self, dep: Dependency) {
        self.prefer_patch_deps
            .entry(dep.package_name())
            .or_insert_with(HashSet::new)
            .insert(dep);
    }

    pub fn version_ordering(&mut self, ordering: VersionOrdering) {
        self.version_ordering = ordering;
    }

    pub fn rust_versions(&mut self, vers: Vec<PartialVersion>) {
        self.rust_versions = vers;
    }

    pub fn publish_time(&mut self, publish_time: jiff::Timestamp) {
        self.publish_time = Some(publish_time);
    }

    pub fn publish_age(&mut self, policy: PublishAgePolicy) {
        self.publish_age = Some(policy);
    }

    /// Returns the version's publish-age if it is too new for the configured
    /// `min-publish-age`, otherwise `None`.
    pub fn too_new(&self, summary: &Summary) -> Option<jiff::SignedDuration> {
        self.publish_age.as_ref()?.too_new(summary)
    }

    /// Whether the given package is preferred.
    pub fn should_prefer(&self, pkg_id: &PackageId) -> bool {
        self.try_to_use.contains(pkg_id)
            || self
                .prefer_patch_deps
                .get(&pkg_id.name())
                .map(|deps| deps.iter().any(|d| d.matches_id(*pkg_id)))
                .unwrap_or(false)
    }

    /// Sort (and filter) the given vector of summaries in-place
    ///
    /// Note: all summaries presumed to be for the same package.
    ///
    /// Sort order:
    /// 1. Preferred packages
    /// 2. Most compatible [`VersionPreferences::rust_versions`]
    /// 3. `first_version`, falling back to [`VersionPreferences::version_ordering`] when `None`
    ///
    /// Filtering:
    /// - `publish_time`
    /// - `first_version`
    pub fn sort_summaries(
        &self,
        summaries: &mut Vec<Summary>,
        first_version: Option<VersionOrdering>,
    ) {
        if let Some(max_publish_time) = self.publish_time {
            summaries.retain(|s| {
                if let Some(summary_publish_time) = s.pubtime() {
                    summary_publish_time <= max_publish_time
                } else {
                    true
                }
            });
        }
        summaries.sort_unstable_by(|a, b| {
            let prefer_a = self.should_prefer(&a.package_id());
            let prefer_b = self.should_prefer(&b.package_id());
            let previous_cmp = prefer_a.cmp(&prefer_b).reverse();
            if previous_cmp != Ordering::Equal {
                return previous_cmp;
            }

            if !self.rust_versions.is_empty() {
                let a_compat_count = self.msrv_compat_count(a);
                let b_compat_count = self.msrv_compat_count(b);
                if b_compat_count != a_compat_count {
                    return b_compat_count.cmp(&a_compat_count);
                }
            }

            let cmp = a.version().cmp(b.version());
            match first_version.unwrap_or(self.version_ordering) {
                VersionOrdering::MaximumVersionsFirst => cmp.reverse(),
                VersionOrdering::MinimumVersionsFirst => cmp,
            }
        });
        if first_version.is_some() && !summaries.is_empty() {
            let _ = summaries.split_off(1);
        }
    }

    fn msrv_compat_count(&self, summary: &Summary) -> usize {
        let Some(rust_version) = summary.rust_version() else {
            return self.rust_versions.len();
        };

        self.rust_versions
            .iter()
            .filter(|max| rust_version.is_compatible_with(max))
            .count()
    }
}

/// Snapshot of the `min-publish-age` configuration before resolution started.
#[derive(Debug)]
pub struct PublishAgePolicy {
    /// Reference "now" from [`GlobalContext::invocation_time`].
    invocation_time: jiff::Timestamp,
    /// `registry.global-min-publish-age`
    global: Option<Duration>,
    /// `registry.min-publish-age`
    crates_io: Option<Duration>,
    /// `registries.<name>.min-publish-age`
    per_registry: HashMap<String, Duration>,
}

impl PublishAgePolicy {
    /// Builds the policy from `min-publish-age` configuration.
    ///
    /// Returns `None` when either meets
    ///
    /// * the `-Zmin-publish-age` gate is off
    /// * the resolver is configured to allow pubtime-incompatible versions
    /// * no threshold is configured at all
    pub fn new(gctx: &GlobalContext) -> CargoResult<Option<Self>> {
        if !gctx.cli_unstable().min_publish_age {
            return Ok(None);
        }

        // An explicit `resolver.incompatible-publish-age = "allow"` disables
        // the filter entirely.
        let resolver_config = gctx.get::<Option<CargoResolverConfig>>("resolver")?;
        if resolver_config
            .and_then(|c| c.incompatible_publish_age)
            .is_some_and(|v| v == IncompatiblePublishAge::Allow)
        {
            return Ok(None);
        }

        let parse = |raw: Option<String>| -> Option<Duration> {
            let raw = raw?;
            // A configured value of `"0"` disables the threshold.
            if raw == "0" {
                return None;
            }
            maybe_parse_time_span(&raw)
        };

        let global = parse(gctx.get::<Option<String>>("registry.global-min-publish-age")?);
        let crates_io = parse(gctx.get::<Option<String>>("registry.min-publish-age")?);
        let mut per_registry = HashMap::new();
        if let Some(registries) =
            gctx.get::<Option<HashMap<String, HashMap<String, String>>>>("registries")?
        {
            for (name, table) in registries {
                if let Some(duration) = parse(table.get("min-publish-age").cloned()) {
                    per_registry.insert(name, duration);
                }
            }
        }

        if global.is_none() && crates_io.is_none() && per_registry.is_empty() {
            return Ok(None);
        }

        Ok(Some(Self {
            invocation_time: gctx.invocation_time(),
            global,
            crates_io,
            per_registry,
        }))
    }

    /// Resolves the minimum publish age for a given registry source.
    ///
    /// Priority:
    ///
    /// 1. `registries.<name>.min-publish-age`
    /// 2. `registry.min-publish-age` (default registry)
    /// 3. `registry.global-min-publish-age`
    fn min_age(&self, source_id: SourceId) -> Option<Duration> {
        let specific = if let Some(name) = source_id.alt_registry_key() {
            self.per_registry.get(name).copied()
        } else if source_id.is_crates_io() {
            self.crates_io
        } else {
            None
        };
        specific.or(self.global)
    }

    /// Returns the version's publish-age if it is too new for its registry.
    ///
    /// `None` means  the version is acceptable
    pub fn too_new(&self, summary: &Summary) -> Option<jiff::SignedDuration> {
        let pubtime = summary.pubtime()?;
        let min_age = self.min_age(summary.source_id())?;
        let span = jiff::Span::new().seconds(min_age.as_secs() as i64);
        let max_pubtime = self.invocation_time.checked_sub(span).ok()?;
        (pubtime > max_pubtime).then(|| self.invocation_time.duration_since(pubtime))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::core::SourceId;
    use std::collections::BTreeMap;

    fn pkgid(name: &str, version: &str) -> PackageId {
        let src_id =
            SourceId::from_url("registry+https://github.com/rust-lang/crates.io-index").unwrap();
        PackageId::try_new(name, version, src_id).unwrap()
    }

    fn dep(name: &str, version: &str) -> Dependency {
        let src_id =
            SourceId::from_url("registry+https://github.com/rust-lang/crates.io-index").unwrap();
        Dependency::parse(name, Some(version), src_id).unwrap()
    }

    fn summ(name: &str, version: &str, msrv: Option<&str>) -> Summary {
        let pkg_id = pkgid(name, version);
        let features = BTreeMap::new();
        Summary::new(
            pkg_id,
            Vec::new(),
            &features,
            None::<&String>,
            msrv.map(|m| m.parse().unwrap()),
        )
        .unwrap()
    }

    fn describe(summaries: &Vec<Summary>) -> String {
        let strs: Vec<String> = summaries
            .iter()
            .map(|summary| format!("{}/{}", summary.name(), summary.version()))
            .collect();
        strs.join(", ")
    }

    #[test]
    fn test_prefer_package_id() {
        let mut vp = VersionPreferences::default();
        vp.prefer_package_id(pkgid("foo", "1.2.3"));

        let mut summaries = vec![
            summ("foo", "1.2.4", None),
            summ("foo", "1.2.3", None),
            summ("foo", "1.1.0", None),
            summ("foo", "1.0.9", None),
        ];

        vp.version_ordering(VersionOrdering::MaximumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.3, foo/1.2.4, foo/1.1.0, foo/1.0.9".to_string()
        );

        vp.version_ordering(VersionOrdering::MinimumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.3, foo/1.0.9, foo/1.1.0, foo/1.2.4".to_string()
        );
    }

    #[test]
    fn test_prefer_dependency() {
        let mut vp = VersionPreferences::default();
        vp.prefer_dependency(dep("foo", "=1.2.3"));

        let mut summaries = vec![
            summ("foo", "1.2.4", None),
            summ("foo", "1.2.3", None),
            summ("foo", "1.1.0", None),
            summ("foo", "1.0.9", None),
        ];

        vp.version_ordering(VersionOrdering::MaximumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.3, foo/1.2.4, foo/1.1.0, foo/1.0.9".to_string()
        );

        vp.version_ordering(VersionOrdering::MinimumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.3, foo/1.0.9, foo/1.1.0, foo/1.2.4".to_string()
        );
    }

    #[test]
    fn test_prefer_both() {
        let mut vp = VersionPreferences::default();
        vp.prefer_package_id(pkgid("foo", "1.2.3"));
        vp.prefer_dependency(dep("foo", "=1.1.0"));

        let mut summaries = vec![
            summ("foo", "1.2.4", None),
            summ("foo", "1.2.3", None),
            summ("foo", "1.1.0", None),
            summ("foo", "1.0.9", None),
        ];

        vp.version_ordering(VersionOrdering::MaximumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.3, foo/1.1.0, foo/1.2.4, foo/1.0.9".to_string()
        );

        vp.version_ordering(VersionOrdering::MinimumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.1.0, foo/1.2.3, foo/1.0.9, foo/1.2.4".to_string()
        );
    }

    #[test]
    fn test_single_rust_version() {
        let mut vp = VersionPreferences::default();
        vp.rust_versions(vec!["1.50".parse().unwrap()]);

        let mut summaries = vec![
            summ("foo", "1.2.4", None),
            summ("foo", "1.2.3", Some("1.60")),
            summ("foo", "1.2.2", None),
            summ("foo", "1.2.1", Some("1.50")),
            summ("foo", "1.2.0", None),
            summ("foo", "1.1.0", Some("1.40")),
            summ("foo", "1.0.9", None),
        ];

        vp.version_ordering(VersionOrdering::MaximumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.4, foo/1.2.2, foo/1.2.1, foo/1.2.0, foo/1.1.0, foo/1.0.9, foo/1.2.3"
                .to_string()
        );

        vp.version_ordering(VersionOrdering::MinimumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.0.9, foo/1.1.0, foo/1.2.0, foo/1.2.1, foo/1.2.2, foo/1.2.4, foo/1.2.3"
                .to_string()
        );
    }

    #[test]
    fn test_multiple_rust_versions() {
        let mut vp = VersionPreferences::default();
        vp.rust_versions(vec!["1.45".parse().unwrap(), "1.55".parse().unwrap()]);

        let mut summaries = vec![
            summ("foo", "1.2.4", None),
            summ("foo", "1.2.3", Some("1.60")),
            summ("foo", "1.2.2", None),
            summ("foo", "1.2.1", Some("1.50")),
            summ("foo", "1.2.0", None),
            summ("foo", "1.1.0", Some("1.40")),
            summ("foo", "1.0.9", None),
        ];

        vp.version_ordering(VersionOrdering::MaximumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.2.4, foo/1.2.2, foo/1.2.0, foo/1.1.0, foo/1.0.9, foo/1.2.1, foo/1.2.3"
                .to_string()
        );

        vp.version_ordering(VersionOrdering::MinimumVersionsFirst);
        vp.sort_summaries(&mut summaries, None);
        assert_eq!(
            describe(&summaries),
            "foo/1.0.9, foo/1.1.0, foo/1.2.0, foo/1.2.2, foo/1.2.4, foo/1.2.1, foo/1.2.3"
                .to_string()
        );
    }

    #[test]
    fn test_empty_summaries() {
        let vp = VersionPreferences::default();
        let mut summaries = vec![];

        vp.sort_summaries(&mut summaries, Some(VersionOrdering::MaximumVersionsFirst));
        assert_eq!(summaries, vec![]);
    }
}
