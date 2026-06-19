//! Error-reporting bridge between PubGrub and Cargo's resolver errors.
//!
//! This module is deliberately self-contained: it is the *only* place that
//! turns a PubGrub failure into a Cargo [`ResolveError`], and it does so by
//! reusing the v1 resolver's own message rendering
//! ([`RequirementError::into_activate_error`]) rather than re-implementing it.
//! Keeping the translation here means the rest of the PubGrub resolver never
//! formats user-facing prose, and the layer can be dropped or rewritten without
//! touching resolution logic.
//!
//! # How reasons flow
//!
//! When [`super::provider::Provider::get_dependencies`] decides a package is
//! unusable it returns [`pubgrub::Dependencies::Unavailable`] carrying an
//! [`UnavailableReason`] (PubGrub's custom incompatibility metadata `M`). That
//! reason lands in the derivation tree as an [`pubgrub::report::External::Custom`]
//! leaf, where [`report_error`] can recover it and render Cargo-native text.

use std::fmt;

use pubgrub::{DefaultStringReporter, DerivationTree, External, PubGrubError, Reporter};

use crate::core::resolver::dep_cache::RequirementError;
use crate::core::resolver::errors::{
    ActivateError, ResolveError, describe_path, no_candidates_error,
};
use crate::core::{Dependency, Registry};

use super::package::PubGrubPackage;
use super::provider::Provider;
use super::semver_pubgrub::SemverPubgrub;

/// PubGrub's custom incompatibility metadata (the `M` type).
///
/// Rather than baking a prose string at the throw site, the provider records
/// *why* a package is unusable in a structured form, so [`report_error`] can map
/// it back to Cargo's own error rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    /// No version of the package satisfied the request (e.g. the exact version
    /// the solution asked for is not in the registry).
    NoVersion,
    /// A feature/dependency requirement could not be met. Carries the v1
    /// resolver's own [`RequirementError`] so the message is rendered
    /// identically.
    Requirement(RequirementError),
}

impl fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Matches the legacy provider string so any fallback rendering is
            // unchanged.
            UnavailableReason::NoVersion => write!(f, "no such version"),
            UnavailableReason::Requirement(req) => write!(f, "{req}"),
        }
    }
}

/// Turn a PubGrub error into a Cargo [`ResolveError`].
///
/// For the common shape — a single [`External::Custom`] leaf carrying an
/// [`UnavailableReason::Requirement`] — this reproduces the v1 resolver's exact
/// message via [`RequirementError::into_activate_error`]. Anything else falls
/// back to PubGrub's [`DefaultStringReporter`], wrapped so the resolution
/// outcome is still a typed `ResolveError`.
pub(super) fn report_error<T: Registry>(
    provider: &Provider<'_, T>,
    err: PubGrubError<Provider<'_, T>>,
) -> anyhow::Error {
    match err {
        PubGrubError::NoSolution(mut derivation_tree) => {
            derivation_tree.collapse_no_versions();
            let package_path = package_path(provider, &derivation_tree);
            if let Some(err) = native_error(provider, &derivation_tree, &package_path) {
                return err.into();
            }
            if let Some(err) = no_candidates_native(provider, &derivation_tree) {
                return err.into();
            }
            // Fallback: PubGrub's own rendering, still surfaced as a
            // `ResolveError` so callers that downcast keep working.
            ResolveError::new(
                anyhow::anyhow!(
                    "failed to select a version for the requirement\n{}",
                    DefaultStringReporter::report(&derivation_tree)
                ),
                package_path,
            )
            .into()
        }
        other => anyhow::anyhow!("pubgrub resolution failed: {other}"),
    }
}

/// Try to render a Cargo-native error for the recognized single-cause shapes.
///
/// Returns `None` when the tree is not a shape we translate, so the caller can
/// fall back to PubGrub's reporter.
fn native_error<T: Registry>(
    provider: &Provider<'_, T>,
    tree: &DerivationTree<PubGrubPackage, SemverPubgrub, UnavailableReason>,
    package_path: &[crate::core::PackageId],
) -> Option<ResolveError> {
    let (pkg, reason) = single_custom_leaf(tree)?;
    let UnavailableReason::Requirement(req) = reason else {
        return None;
    };
    // Recover the failing package's summary so the v1 renderer can build the
    // exact message (feature lists, "dep:" help text, etc.).
    let name = pkg.base_name()?;
    let summary = provider.any_summary(name.name, name.source)?;
    match req.clone().into_activate_error(None, &summary) {
        ActivateError::Fatal(e) => Some(ResolveError::new(e, package_path.to_vec())),
        // The `parent: None` arms of `into_activate_error` only ever produce
        // `Fatal`, so a `Conflict` here means our assumption broke; fall back.
        ActivateError::Conflict(..) => None,
    }
}

/// Render the "no candidates found" family (no matching package / version /
/// yanked / typo) by reconstructing the failing dependency and its parent from
/// the derivation tree, then delegating to the v1 resolver's
/// [`no_candidates_error`] for byte-identical text.
///
/// Returns `None` if the tree is not a recognizable "parent depends on a
/// missing child" shape, so the caller can fall back to PubGrub's reporter.
fn no_candidates_native<T: Registry>(
    provider: &Provider<'_, T>,
    tree: &DerivationTree<PubGrubPackage, SemverPubgrub, UnavailableReason>,
) -> Option<ResolveError> {
    // Find a `parent depends on dep` edge where no candidate satisfies `dep`.
    let (parent_id, dep) = unsatisfiable_dependency(provider, tree)?;
    let required_by = describe_path(std::iter::once((&parent_id, None)));
    let registry = provider.registry();
    Some(no_candidates_error(
        registry.registry(),
        &dep,
        provider.version_prefs(),
        vec![parent_id],
        &required_by,
        // The provider does not carry a `GlobalContext`, so the offline-mode
        // hint is omitted; it only adds an advisory note.
        None,
    ))
}

/// Find a dependency edge in the tree where the depended-on crate has **no
/// candidate version satisfying the requirement** — either the crate is absent
/// entirely or every published version is out of range.
///
/// Returns the parent's resolved [`PackageId`] and the original [`Dependency`].
/// This deliberately excludes the "some candidate matches but conflicts with
/// another selection" case (handled by the conflict branch, not here), so the
/// caller only produces the "no candidates" message when it is truly accurate.
fn unsatisfiable_dependency<T: Registry>(
    provider: &Provider<'_, T>,
    tree: &DerivationTree<PubGrubPackage, SemverPubgrub, UnavailableReason>,
) -> Option<(crate::core::PackageId, Dependency)> {
    match tree {
        DerivationTree::External(External::FromDependencyOf(parent, _, child, _)) => {
            let parent_name = parent.base_name()?;
            let parent_summary = provider.any_summary(parent_name.name, parent_name.source)?;
            let child_name = child.base_name()?;
            // Recover the original `Dependency` from the parent's manifest.
            let dep = parent_summary
                .dependencies()
                .iter()
                .find(|d| {
                    d.package_name() == child_name.name && d.source_id() == child_name.source
                })?
                .clone();
            // Only claim "no candidates" when nothing actually matches the req.
            let satisfiable = provider
                .matching_summaries(&dep)
                .is_some_and(|summaries| !summaries.is_empty());
            if satisfiable {
                return None;
            }
            Some((parent_summary.package_id(), dep))
        }
        DerivationTree::Derived(derived) => unsatisfiable_dependency(provider, &derived.cause1)
            .or_else(|| unsatisfiable_dependency(provider, &derived.cause2)),
        _ => None,
    }
}

/// Best-effort reconstruction of the [`ResolveError`] package path: the
/// workspace member(s) whose requirements led to the failure.
///
/// The default resolver reports the path from the failing package up to the
/// root. PubGrub's derivation tree does not preserve that ordering, but the
/// workspace members referenced in the tree are recoverable and are what
/// consumers (e.g. `cargo metadata`/member diagnostics) key on.
fn package_path<T: Registry>(
    provider: &Provider<'_, T>,
    tree: &DerivationTree<PubGrubPackage, SemverPubgrub, UnavailableReason>,
) -> Vec<crate::core::PackageId> {
    let mut members = Vec::new();
    collect_members(provider, tree, &mut members);
    members
}

/// Collect the resolved [`PackageId`]s of member `Bucket` packages in the tree.
fn collect_members<T: Registry>(
    provider: &Provider<'_, T>,
    tree: &DerivationTree<PubGrubPackage, SemverPubgrub, UnavailableReason>,
    out: &mut Vec<crate::core::PackageId>,
) {
    match tree {
        DerivationTree::External(ext) => {
            for pkg in external_packages(ext) {
                if let PubGrubPackage::Bucket {
                    name, member: true, ..
                } = pkg
                {
                    if let Some(summary) = provider.any_summary(name.name, name.source) {
                        let id = summary.package_id();
                        if !out.contains(&id) {
                            out.push(id);
                        }
                    }
                }
            }
        }
        DerivationTree::Derived(derived) => {
            collect_members(provider, &derived.cause1, out);
            collect_members(provider, &derived.cause2, out);
        }
    }
}

/// The packages referenced by an [`External`] incompatibility.
fn external_packages(
    ext: &External<PubGrubPackage, SemverPubgrub, UnavailableReason>,
) -> Vec<&PubGrubPackage> {
    match ext {
        External::NotRoot(p, _) | External::NoVersions(p, _) | External::Custom(p, _, _) => {
            vec![p]
        }
        External::FromDependencyOf(p1, _, p2, _) => vec![p1, p2],
    }
}

/// If the whole derivation tree reduces to a single [`External::Custom`] leaf,
/// return its package and reason.
fn single_custom_leaf<'a>(
    tree: &'a DerivationTree<PubGrubPackage, SemverPubgrub, UnavailableReason>,
) -> Option<(&'a PubGrubPackage, &'a UnavailableReason)> {
    match tree {
        DerivationTree::External(External::Custom(pkg, _, reason)) => Some((pkg, reason)),
        DerivationTree::Derived(derived) => {
            // Walk through single-cause derivations (the other cause being a
            // trivially-true "not root" / dependency-of link).
            let c1 = single_custom_leaf(&derived.cause1);
            let c2 = single_custom_leaf(&derived.cause2);
            match (c1, c2) {
                (Some(found), None) | (None, Some(found)) => Some(found),
                _ => None,
            }
        }
        _ => None,
    }
}
