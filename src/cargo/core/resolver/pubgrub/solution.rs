//! Reconstruct a Cargo [`Resolve`] from a PubGrub solution.
//!
//! PubGrub returns a [`SelectedDependencies`] mapping each [`PubGrubPackage`] to
//! the version it selected. We project that back onto Cargo's model:
//!
//! * concrete [`PubGrubPackage::Bucket`] packages become the resolved
//!   [`PackageId`]s and graph nodes;
//! * the feature/default-feature packages tell us which features each package
//!   ended up with;
//! * graph edges are recovered by walking each resolved summary's
//!   dependencies, keeping the ones that the feature solution activated, and
//!   linking them to the selected child version.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use semver::Version;

use pubgrub::SelectedDependencies;

use crate::core::dependency::DepKind;
use crate::core::resolver::Resolve;
use crate::core::resolver::ResolveVersion;
use crate::core::{Dependency, PackageId, Registry, SourceId, Summary};
use crate::util::Graph;
use crate::util::errors::CargoResult;
use crate::util::interning::{INTERNED_DEFAULT, InternedString};

use super::package::{BucketName, FeatureNamespace, PubGrubPackage};
use super::provider::Provider;
use super::semver_pubgrub::SemverCompatibility;

/// Per-package activation facts gathered from the PubGrub solution.
#[derive(Default)]
struct Activation {
    /// Activated named features (including `default`).
    features: BTreeSet<InternedString>,
    /// Activated optional dependencies (the toml names that appeared as
    /// `BucketFeatures{.., Dep(name)}` in the solution).
    deps: HashSet<InternedString>,
    /// Whether this package was resolved as a workspace member (dev-deps).
    member: bool,
}

pub(super) fn into_resolve<T: Registry>(
    provider: &Provider<'_, T>,
    solution: &SelectedDependencies<PubGrubPackage, Version>,
    resolve_version: ResolveVersion,
) -> CargoResult<Resolve> {
    // (name, source) -> selected packages (one per compatibility bucket). The
    // key is the *bucket* identity (the source the dependency named), while the
    // value is the resolved [`PackageId`], whose source may differ when the
    // package was redirected by `[patch]` (e.g. a `crates-io` dep satisfied by
    // a path patch). See [`bucket_pid`].
    let mut selected: HashMap<(InternedString, SourceId), BTreeSet<PackageId>> = HashMap::new();
    // PackageId -> activation facts.
    let mut activations: HashMap<PackageId, Activation> = HashMap::new();
    let mut package_ids: BTreeSet<PackageId> = BTreeSet::new();
    // Resolved summary per node. Captured here (keyed by the patched
    // [`PackageId`]) so later passes don't re-query by a source that no longer
    // matches the queryer's bucket cache.
    let mut summaries: HashMap<PackageId, Summary> = HashMap::new();

    for (pkg, version) in solution.iter() {
        match pkg {
            PubGrubPackage::Bucket {
                name,
                member,
                all_features: _,
            } => {
                let (pid, summary) = bucket_pid(provider, name, version)?;
                package_ids.insert(pid);
                summaries.insert(pid, summary);
                selected
                    .entry((name.name, name.source))
                    .or_default()
                    .insert(pid);
                let act = activations.entry(pid).or_default();
                act.member |= *member;
            }
            PubGrubPackage::BucketFeatures { name, feature } => {
                let (pid, _) = bucket_pid(provider, name, version)?;
                let act = activations.entry(pid).or_default();
                match feature {
                    FeatureNamespace::Feat(f) => {
                        act.features.insert(*f);
                    }
                    // Optional-dependency activations don't contribute to the
                    // user-facing feature list, but do gate optional edges.
                    FeatureNamespace::Dep(d) => {
                        act.deps.insert(*d);
                    }
                }
            }
            PubGrubPackage::BucketDefaultFeatures { name } => {
                let (pid, _) = bucket_pid(provider, name, version)?;
                activations
                    .entry(pid)
                    .or_default()
                    .features
                    .insert(INTERNED_DEFAULT);
            }
            // Wide/links/root packages are not real graph nodes.
            PubGrubPackage::Root
            | PubGrubPackage::Wide { .. }
            | PubGrubPackage::WideFeatures { .. }
            | PubGrubPackage::WideDefaultFeatures { .. }
            | PubGrubPackage::Links { .. } => {}
        }
    }

    // Build the dependency graph.
    let mut graph: Graph<PackageId, HashSet<Dependency>> = Graph::new();
    for pid in &package_ids {
        graph.add(*pid);
    }

    for pid in &package_ids {
        let Some(summary) = summaries.get(pid) else {
            anyhow::bail!("pubgrub selected `{pid}` but it has no summary");
        };
        let act = activations.get(pid);
        let member = act.is_some_and(|a| a.member);
        for dep in summary.dependencies() {
            // Determine whether this dependency is part of the resolved graph:
            //
            // * dev-dependencies are only recorded for workspace members;
            // * optional dependencies are recorded only when activated (some
            //   feature turned them on), so that unactivated optional deps do
            //   not introduce spurious edges (and cycles);
            // * all other dependencies are always recorded.
            let active = match dep.kind() {
                DepKind::Development => member,
                _ => {
                    !dep.is_optional() || act.is_some_and(|a| a.deps.contains(&dep.name_in_toml()))
                }
            };
            if !active {
                continue;
            }
            let Some(child) = resolve_child(provider, dep, pid, solution, &selected) else {
                // An active dependency with no resolved child indicates a bug
                // in the encoding rather than a benign skip.
                anyhow::bail!(
                    "pubgrub could not map dependency `{}` of `{pid}` to a resolved package",
                    dep.package_name()
                );
            };
            graph.link(*pid, child).insert(dep.clone());
        }
    }

    // Checksums, features and replacements.
    let mut cksums = HashMap::new();
    let mut features: HashMap<PackageId, Vec<InternedString>> = HashMap::new();
    let mut replacements = HashMap::new();
    {
        let registry = provider.registry();
        for pid in &package_ids {
            let summary = &summaries[pid];
            cksums.insert(*pid, summary.checksum().map(|s| s.to_string()));
            if let Some((from, to)) = registry.used_replacement_for(*pid) {
                replacements.insert(from, to);
            }
            if let Some(act) = activations.get(pid) {
                let mut feats: Vec<InternedString> = act.features.iter().copied().collect();
                feats.sort_unstable();
                features.insert(*pid, feats);
            }
        }
    }

    let resolve = Resolve::new(
        graph,
        replacements,
        features,
        cksums,
        BTreeMap::new(),
        Vec::new(),
        resolve_version,
        summaries,
    );

    super::super::check_cycles(&resolve)?;
    super::super::check_duplicate_pkgs_in_lockfile(&resolve)?;
    Ok(resolve)
}

/// Find the resolved child [`PackageId`] that satisfies `dep` from `parent`.
///
/// The lookup is keyed by the *bucket* `(name, source)` the dependency named,
/// but the returned [`PackageId`] is the one actually selected for that bucket,
/// whose source may differ when the dependency was redirected by `[patch]`.
fn resolve_child<T: Registry>(
    provider: &Provider<'_, T>,
    dep: &Dependency,
    parent: &PackageId,
    solution: &SelectedDependencies<PubGrubPackage, Version>,
    selected: &HashMap<(InternedString, SourceId), BTreeSet<PackageId>>,
) -> Option<PackageId> {
    let (cray, _) = provider.from_dep(dep, parent.name(), parent.version());
    let (name, source, compat) = match cray {
        PubGrubPackage::Bucket { ref name, .. } => (name.name, name.source, name.compat),
        PubGrubPackage::Wide { ref name } => {
            // The wide package chose a bucket; read it from the solution.
            let chosen = solution.get(&cray)?;
            (name.name, name.source, SemverCompatibility::from(chosen))
        }
        _ => return None,
    };
    let pids = selected.get(&(name, source))?;
    pids.iter()
        .find(|pid| SemverCompatibility::from(pid.version()) == compat)
        .copied()
}

/// Resolve a [`BucketName`] + version to the selected package and its summary.
///
/// The bucket names a `(crate, source)`, but `[patch]` can redirect a query to a
/// summary from a *different* source (e.g. a `crates-io` requirement satisfied
/// by a path patch). [`Provider::summary_for`] returns that real summary; we use
/// its [`PackageId`] — carrying the patched source — as the node identity, so
/// the lockfile records the package the patch actually provided rather than the
/// nominal registry source.
fn bucket_pid<T: Registry>(
    provider: &Provider<'_, T>,
    name: &BucketName,
    version: &Version,
) -> CargoResult<(PackageId, Summary)> {
    let Some(summary) = provider.summary_for(name.name, name.source, version)? else {
        anyhow::bail!(
            "pubgrub selected `{} {}` from `{}` but it has no summary",
            name.name,
            version,
            name.source
        );
    };
    Ok((summary.package_id(), summary))
}
