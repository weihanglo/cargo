use annotate_snippets::{Level, Renderer, Snippet};
use cargo_util_schemas::core::PatchInfo;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::{self, FromStr};

use crate::AlreadyPrintedError;
use anyhow::{anyhow, bail, Context as _};
use cargo_platform::Platform;
use cargo_util::paths::{self, normalize_path};
use cargo_util_schemas::manifest::{self, TomlManifest};
use cargo_util_schemas::manifest::{RustVersion, StringOrBool};
use itertools::Itertools;
use lazycell::LazyCell;
use pathdiff::diff_paths;
use url::Url;

use crate::core::compiler::{CompileKind, CompileTarget};
use crate::core::dependency::{Artifact, ArtifactTarget, DepKind};
use crate::core::manifest::{ManifestMetadata, TargetSourcePath};
use crate::core::resolver::ResolveBehavior;
use crate::core::{find_workspace_root, resolve_relative_path, CliUnstable, FeatureValue};
use crate::core::{Dependency, Manifest, Package, PackageId, Summary, Target};
use crate::core::{Edition, EitherManifest, Feature, Features, VirtualManifest, Workspace};
use crate::core::{GitReference, PackageIdSpec, SourceId, WorkspaceConfig, WorkspaceRootConfig};
use crate::sources::{CRATES_IO_INDEX, CRATES_IO_REGISTRY};
use crate::util::errors::{CargoResult, ManifestError};
use crate::util::interning::InternedString;
use crate::util::CanonicalUrl;
use crate::util::{self, context::ConfigRelativePath, GlobalContext, IntoUrl, OptVersionReq};

mod embedded;
mod targets;

use self::targets::to_targets;

/// See also `bin/cargo/commands/run.rs`s `is_manifest_command`
pub fn is_embedded(path: &Path) -> bool {
    let ext = path.extension();
    ext == Some(OsStr::new("rs")) ||
        // Provide better errors by not considering directories to be embedded manifests
        (ext.is_none() && path.is_file())
}

/// Loads a `Cargo.toml` from a file on disk.
///
/// This could result in a real or virtual manifest being returned.
///
/// A list of nested paths is also returned, one for each path dependency
/// within the manifest. For virtual manifests, these paths can only
/// come from patched or replaced dependencies. These paths are not
/// canonicalized.
#[tracing::instrument(skip(gctx))]
pub fn read_manifest(
    path: &Path,
    source_id: SourceId,
    gctx: &GlobalContext,
) -> CargoResult<EitherManifest> {
    let mut warnings = Default::default();
    let mut errors = Default::default();

    let contents =
        read_toml_string(path, gctx).map_err(|err| ManifestError::new(err, path.into()))?;
    let document =
        parse_document(&contents).map_err(|e| emit_diagnostic(e.into(), &contents, path, gctx))?;
    let original_toml = deserialize_toml(&document)
        .map_err(|e| emit_diagnostic(e.into(), &contents, path, gctx))?;

    let mut manifest = (|| {
        let empty = Vec::new();
        let cargo_features = original_toml.cargo_features.as_ref().unwrap_or(&empty);
        let features = Features::new(cargo_features, gctx, &mut warnings, source_id.is_path())?;
        let workspace_config = to_workspace_config(&original_toml, path, gctx, &mut warnings)?;
        if let WorkspaceConfig::Root(ws_root_config) = &workspace_config {
            let package_root = path.parent().unwrap();
            gctx.ws_roots
                .borrow_mut()
                .insert(package_root.to_owned(), ws_root_config.clone());
        }
        let resolved_toml = resolve_toml(
            &original_toml,
            &features,
            &workspace_config,
            path,
            gctx,
            &mut warnings,
            &mut errors,
        )?;

        if resolved_toml.package().is_some() {
            to_real_manifest(
                contents,
                document,
                original_toml,
                resolved_toml,
                features,
                workspace_config,
                source_id,
                path,
                gctx,
                &mut warnings,
                &mut errors,
            )
            .map(EitherManifest::Real)
        } else {
            to_virtual_manifest(
                contents,
                document,
                original_toml,
                resolved_toml,
                features,
                workspace_config,
                source_id,
                path,
                gctx,
                &mut warnings,
                &mut errors,
            )
            .map(EitherManifest::Virtual)
        }
    })()
    .map_err(|err| {
        ManifestError::new(
            err.context(format!("failed to parse manifest at `{}`", path.display())),
            path.into(),
        )
    })?;

    for warning in warnings {
        manifest.warnings_mut().add_warning(warning);
    }
    for error in errors {
        manifest.warnings_mut().add_critical_warning(error);
    }

    Ok(manifest)
}

#[tracing::instrument(skip_all)]
fn read_toml_string(path: &Path, gctx: &GlobalContext) -> CargoResult<String> {
    let mut contents = paths::read(path)?;
    if is_embedded(path) {
        if !gctx.cli_unstable().script {
            anyhow::bail!("parsing `{}` requires `-Zscript`", path.display());
        }
        contents = embedded::expand_manifest(&contents, path, gctx)?;
    }
    Ok(contents)
}

#[tracing::instrument(skip_all)]
fn parse_document(contents: &str) -> Result<toml_edit::ImDocument<String>, toml_edit::de::Error> {
    toml_edit::ImDocument::parse(contents.to_owned()).map_err(Into::into)
}

#[tracing::instrument(skip_all)]
fn deserialize_toml(
    document: &toml_edit::ImDocument<String>,
) -> Result<manifest::TomlManifest, toml_edit::de::Error> {
    let mut unused = BTreeSet::new();
    let deserializer = toml_edit::de::Deserializer::from(document.clone());
    let mut document: manifest::TomlManifest = serde_ignored::deserialize(deserializer, |path| {
        let mut key = String::new();
        stringify(&mut key, &path);
        unused.insert(key);
    })?;
    document._unused_keys = unused;
    Ok(document)
}

fn stringify(dst: &mut String, path: &serde_ignored::Path<'_>) {
    use serde_ignored::Path;

    match *path {
        Path::Root => {}
        Path::Seq { parent, index } => {
            stringify(dst, parent);
            if !dst.is_empty() {
                dst.push('.');
            }
            dst.push_str(&index.to_string());
        }
        Path::Map { parent, ref key } => {
            stringify(dst, parent);
            if !dst.is_empty() {
                dst.push('.');
            }
            dst.push_str(key);
        }
        Path::Some { parent }
        | Path::NewtypeVariant { parent }
        | Path::NewtypeStruct { parent } => stringify(dst, parent),
    }
}

fn to_workspace_config(
    original_toml: &manifest::TomlManifest,
    manifest_file: &Path,
    gctx: &GlobalContext,
    warnings: &mut Vec<String>,
) -> CargoResult<WorkspaceConfig> {
    let workspace_config = match (
        original_toml.workspace.as_ref(),
        original_toml.package().and_then(|p| p.workspace.as_ref()),
    ) {
        (Some(toml_config), None) => {
            verify_lints(toml_config.lints.as_ref(), gctx, warnings)?;
            if let Some(ws_deps) = &toml_config.dependencies {
                for (name, dep) in ws_deps {
                    if dep.is_optional() {
                        bail!("{name} is optional, but workspace dependencies cannot be optional",);
                    }
                    if dep.is_public() {
                        bail!("{name} is public, but workspace dependencies cannot be public",);
                    }
                }

                for (name, dep) in ws_deps {
                    unused_dep_keys(name, "workspace.dependencies", dep.unused_keys(), warnings);
                }
            }
            let ws_root_config = to_workspace_root_config(toml_config, manifest_file);
            WorkspaceConfig::Root(ws_root_config)
        }
        (None, root) => WorkspaceConfig::Member {
            root: root.cloned(),
        },
        (Some(..), Some(..)) => bail!(
            "cannot configure both `package.workspace` and \
                 `[workspace]`, only one can be specified"
        ),
    };
    Ok(workspace_config)
}

fn to_workspace_root_config(
    resolved_toml: &manifest::TomlWorkspace,
    manifest_file: &Path,
) -> WorkspaceRootConfig {
    let package_root = manifest_file.parent().unwrap();
    let inheritable = InheritableFields {
        package: resolved_toml.package.clone(),
        dependencies: resolved_toml.dependencies.clone(),
        lints: resolved_toml.lints.clone(),
        _ws_root: package_root.to_owned(),
    };
    let ws_root_config = WorkspaceRootConfig::new(
        package_root,
        &resolved_toml.members,
        &resolved_toml.default_members,
        &resolved_toml.exclude,
        &Some(inheritable),
        &resolved_toml.metadata,
    );
    ws_root_config
}

#[tracing::instrument(skip_all)]
fn resolve_toml(
    original_toml: &manifest::TomlManifest,
    features: &Features,
    workspace_config: &WorkspaceConfig,
    manifest_file: &Path,
    gctx: &GlobalContext,
    warnings: &mut Vec<String>,
    _errors: &mut Vec<String>,
) -> CargoResult<manifest::TomlManifest> {
    let mut resolved_toml = manifest::TomlManifest {
        cargo_features: original_toml.cargo_features.clone(),
        package: None,
        project: None,
        profile: original_toml.profile.clone(),
        lib: original_toml.lib.clone(),
        bin: original_toml.bin.clone(),
        example: original_toml.example.clone(),
        test: original_toml.test.clone(),
        bench: original_toml.bench.clone(),
        dependencies: None,
        dev_dependencies: None,
        dev_dependencies2: None,
        build_dependencies: None,
        build_dependencies2: None,
        features: original_toml.features.clone(),
        target: None,
        replace: original_toml.replace.clone(),
        patch: original_toml.patch.clone(),
        workspace: original_toml.workspace.clone(),
        badges: None,
        lints: None,
        _unused_keys: Default::default(),
    };

    let package_root = manifest_file.parent().unwrap();

    let inherit_cell: LazyCell<InheritableFields> = LazyCell::new();
    let inherit = || {
        inherit_cell
            .try_borrow_with(|| load_inheritable_fields(gctx, manifest_file, &workspace_config))
    };

    if let Some(original_package) = original_toml.package() {
        let resolved_package = resolve_package_toml(original_package, package_root, &inherit)?;
        resolved_toml.package = Some(resolved_package);

        resolved_toml.dependencies = resolve_dependencies(
            gctx,
            &features,
            original_toml.dependencies.as_ref(),
            None,
            &inherit,
            package_root,
            warnings,
        )?;
        resolved_toml.dev_dependencies = resolve_dependencies(
            gctx,
            &features,
            original_toml.dev_dependencies(),
            Some(DepKind::Development),
            &inherit,
            package_root,
            warnings,
        )?;
        resolved_toml.build_dependencies = resolve_dependencies(
            gctx,
            &features,
            original_toml.build_dependencies(),
            Some(DepKind::Build),
            &inherit,
            package_root,
            warnings,
        )?;
        let mut resolved_target = BTreeMap::new();
        for (name, platform) in original_toml.target.iter().flatten() {
            let resolved_dependencies = resolve_dependencies(
                gctx,
                &features,
                platform.dependencies.as_ref(),
                None,
                &inherit,
                package_root,
                warnings,
            )?;
            let resolved_dev_dependencies = resolve_dependencies(
                gctx,
                &features,
                platform.dev_dependencies(),
                Some(DepKind::Development),
                &inherit,
                package_root,
                warnings,
            )?;
            let resolved_build_dependencies = resolve_dependencies(
                gctx,
                &features,
                platform.build_dependencies(),
                Some(DepKind::Build),
                &inherit,
                package_root,
                warnings,
            )?;
            resolved_target.insert(
                name.clone(),
                manifest::TomlPlatform {
                    dependencies: resolved_dependencies,
                    build_dependencies: resolved_build_dependencies,
                    build_dependencies2: None,
                    dev_dependencies: resolved_dev_dependencies,
                    dev_dependencies2: None,
                },
            );
        }
        resolved_toml.target = (!resolved_target.is_empty()).then_some(resolved_target);

        let resolved_lints = original_toml
            .lints
            .clone()
            .map(|value| lints_inherit_with(value, || inherit()?.lints()))
            .transpose()?;
        resolved_toml.lints = resolved_lints.map(|lints| manifest::InheritableLints {
            workspace: false,
            lints,
        });

        let resolved_badges = original_toml
            .badges
            .clone()
            .map(|mw| field_inherit_with(mw, "badges", || inherit()?.badges()))
            .transpose()?;
        resolved_toml.badges = resolved_badges.map(manifest::InheritableField::Value);
    } else {
        for field in original_toml.requires_package() {
            bail!("this virtual manifest specifies a `{field}` section, which is not allowed");
        }
    }

    Ok(resolved_toml)
}

#[tracing::instrument(skip_all)]
fn resolve_package_toml<'a>(
    original_package: &manifest::TomlPackage,
    package_root: &Path,
    inherit: &dyn Fn() -> CargoResult<&'a InheritableFields>,
) -> CargoResult<Box<manifest::TomlPackage>> {
    let resolved_package = manifest::TomlPackage {
        edition: original_package
            .edition
            .clone()
            .map(|value| field_inherit_with(value, "edition", || inherit()?.edition()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        rust_version: original_package
            .rust_version
            .clone()
            .map(|value| field_inherit_with(value, "rust-version", || inherit()?.rust_version()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        name: original_package.name.clone(),
        version: original_package
            .version
            .clone()
            .map(|value| field_inherit_with(value, "version", || inherit()?.version()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        authors: original_package
            .authors
            .clone()
            .map(|value| field_inherit_with(value, "authors", || inherit()?.authors()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        build: original_package.build.clone(),
        metabuild: original_package.metabuild.clone(),
        default_target: original_package.default_target.clone(),
        forced_target: original_package.forced_target.clone(),
        links: original_package.links.clone(),
        exclude: original_package
            .exclude
            .clone()
            .map(|value| field_inherit_with(value, "exclude", || inherit()?.exclude()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        include: original_package
            .include
            .clone()
            .map(|value| field_inherit_with(value, "include", || inherit()?.include()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        publish: original_package
            .publish
            .clone()
            .map(|value| field_inherit_with(value, "publish", || inherit()?.publish()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        workspace: original_package.workspace.clone(),
        im_a_teapot: original_package.im_a_teapot.clone(),
        autobins: original_package.autobins.clone(),
        autoexamples: original_package.autoexamples.clone(),
        autotests: original_package.autotests.clone(),
        autobenches: original_package.autobenches.clone(),
        default_run: original_package.default_run.clone(),
        description: original_package
            .description
            .clone()
            .map(|value| field_inherit_with(value, "description", || inherit()?.description()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        homepage: original_package
            .homepage
            .clone()
            .map(|value| field_inherit_with(value, "homepage", || inherit()?.homepage()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        documentation: original_package
            .documentation
            .clone()
            .map(|value| field_inherit_with(value, "documentation", || inherit()?.documentation()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        readme: resolve_package_readme(
            package_root,
            original_package
                .readme
                .clone()
                .map(|value| {
                    field_inherit_with(value, "readme", || inherit()?.readme(package_root))
                })
                .transpose()?
                .as_ref(),
        )
        .map(|s| manifest::InheritableField::Value(StringOrBool::String(s))),
        keywords: original_package
            .keywords
            .clone()
            .map(|value| field_inherit_with(value, "keywords", || inherit()?.keywords()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        categories: original_package
            .categories
            .clone()
            .map(|value| field_inherit_with(value, "categories", || inherit()?.categories()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        license: original_package
            .license
            .clone()
            .map(|value| field_inherit_with(value, "license", || inherit()?.license()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        license_file: original_package
            .license_file
            .clone()
            .map(|value| {
                field_inherit_with(value, "license-file", || {
                    inherit()?.license_file(package_root)
                })
            })
            .transpose()?
            .map(manifest::InheritableField::Value),
        repository: original_package
            .repository
            .clone()
            .map(|value| field_inherit_with(value, "repository", || inherit()?.repository()))
            .transpose()?
            .map(manifest::InheritableField::Value),
        resolver: original_package.resolver.clone(),
        metadata: original_package.metadata.clone(),
        _invalid_cargo_features: Default::default(),
    };
    Ok(Box::new(resolved_package))
}

/// Returns the name of the README file for a [`manifest::TomlPackage`].
fn resolve_package_readme(
    package_root: &Path,
    readme: Option<&manifest::StringOrBool>,
) -> Option<String> {
    match &readme {
        None => default_readme_from_package_root(package_root),
        Some(value) => match value {
            manifest::StringOrBool::Bool(false) => None,
            manifest::StringOrBool::Bool(true) => Some("README.md".to_string()),
            manifest::StringOrBool::String(v) => Some(v.clone()),
        },
    }
}

const DEFAULT_README_FILES: [&str; 3] = ["README.md", "README.txt", "README"];

/// Checks if a file with any of the default README file names exists in the package root.
/// If so, returns a `String` representing that name.
fn default_readme_from_package_root(package_root: &Path) -> Option<String> {
    for &readme_filename in DEFAULT_README_FILES.iter() {
        if package_root.join(readme_filename).is_file() {
            return Some(readme_filename.to_string());
        }
    }

    None
}

#[tracing::instrument(skip_all)]
fn resolve_dependencies<'a>(
    gctx: &GlobalContext,
    features: &Features,
    orig_deps: Option<&BTreeMap<manifest::PackageName, manifest::InheritableDependency>>,
    kind: Option<DepKind>,
    inherit: &dyn Fn() -> CargoResult<&'a InheritableFields>,
    package_root: &Path,
    warnings: &mut Vec<String>,
) -> CargoResult<Option<BTreeMap<manifest::PackageName, manifest::InheritableDependency>>> {
    let Some(dependencies) = orig_deps else {
        return Ok(None);
    };

    let mut deps = BTreeMap::new();
    for (name_in_toml, v) in dependencies.iter() {
        let mut resolved =
            dependency_inherit_with(v.clone(), name_in_toml, inherit, package_root, warnings)?;
        if let manifest::TomlDependency::Detailed(ref mut d) = resolved {
            if d.public.is_some() {
                let public_feature = features.require(Feature::public_dependency());
                let with_public_feature = public_feature.is_ok();
                let with_z_public = gctx.cli_unstable().public_dependency;
                if !with_public_feature && (!with_z_public && !gctx.nightly_features_allowed) {
                    public_feature?;
                }
                if matches!(kind, None) {
                    if !with_public_feature && !with_z_public {
                        d.public = None;
                        warnings.push(format!(
                            "ignoring `public` on dependency {name_in_toml}, pass `-Zpublic-dependency` to enable support for it"
                        ))
                    }
                } else {
                    let kind_name = match kind {
                        Some(k) => k.kind_table(),
                        None => "dependencies",
                    };
                    let hint = format!(
                        "'public' specifier can only be used on regular dependencies, not {kind_name}",
                    );
                    if with_public_feature || with_z_public {
                        bail!(hint)
                    } else {
                        // If public feature isn't enabled in nightly, we instead warn that.
                        warnings.push(hint);
                        d.public = None;
                    }
                }
            }
        }

        deps.insert(
            name_in_toml.clone(),
            manifest::InheritableDependency::Value(resolved.clone()),
        );
    }
    Ok(Some(deps))
}

fn load_inheritable_fields(
    gctx: &GlobalContext,
    resolved_path: &Path,
    workspace_config: &WorkspaceConfig,
) -> CargoResult<InheritableFields> {
    match workspace_config {
        WorkspaceConfig::Root(root) => Ok(root.inheritable().clone()),
        WorkspaceConfig::Member {
            root: Some(ref path_to_root),
        } => {
            let path = resolved_path
                .parent()
                .unwrap()
                .join(path_to_root)
                .join("Cargo.toml");
            let root_path = paths::normalize_path(&path);
            inheritable_from_path(gctx, root_path)
        }
        WorkspaceConfig::Member { root: None } => {
            match find_workspace_root(&resolved_path, gctx)? {
                Some(path_to_root) => inheritable_from_path(gctx, path_to_root),
                None => Err(anyhow!("failed to find a workspace root")),
            }
        }
    }
}

fn inheritable_from_path(
    gctx: &GlobalContext,
    workspace_path: PathBuf,
) -> CargoResult<InheritableFields> {
    // Workspace path should have Cargo.toml at the end
    let workspace_path_root = workspace_path.parent().unwrap();

    // Let the borrow exit scope so that it can be picked up if there is a need to
    // read a manifest
    if let Some(ws_root) = gctx.ws_roots.borrow().get(workspace_path_root) {
        return Ok(ws_root.inheritable().clone());
    };

    let source_id = SourceId::for_path(workspace_path_root)?;
    let man = read_manifest(&workspace_path, source_id, gctx)?;
    match man.workspace_config() {
        WorkspaceConfig::Root(root) => {
            gctx.ws_roots
                .borrow_mut()
                .insert(workspace_path, root.clone());
            Ok(root.inheritable().clone())
        }
        _ => bail!(
            "root of a workspace inferred but wasn't a root: {}",
            workspace_path.display()
        ),
    }
}

/// Defines simple getter methods for inheritable fields.
macro_rules! package_field_getter {
    ( $(($key:literal, $field:ident -> $ret:ty),)* ) => (
        $(
            #[doc = concat!("Gets the field `workspace.package", $key, "`.")]
            fn $field(&self) -> CargoResult<$ret> {
                let Some(val) = self.package.as_ref().and_then(|p| p.$field.as_ref()) else  {
                    bail!("`workspace.package.{}` was not defined", $key);
                };
                Ok(val.clone())
            }
        )*
    )
}

/// A group of fields that are inheritable by members of the workspace
#[derive(Clone, Debug, Default)]
pub struct InheritableFields {
    package: Option<manifest::InheritablePackage>,
    dependencies: Option<BTreeMap<manifest::PackageName, manifest::TomlDependency>>,
    lints: Option<manifest::TomlLints>,

    // Bookkeeping to help when resolving values from above
    _ws_root: PathBuf,
}

impl InheritableFields {
    package_field_getter! {
        // Please keep this list lexicographically ordered.
        ("authors",       authors       -> Vec<String>),
        ("badges",        badges        -> BTreeMap<String, BTreeMap<String, String>>),
        ("categories",    categories    -> Vec<String>),
        ("description",   description   -> String),
        ("documentation", documentation -> String),
        ("edition",       edition       -> String),
        ("exclude",       exclude       -> Vec<String>),
        ("homepage",      homepage      -> String),
        ("include",       include       -> Vec<String>),
        ("keywords",      keywords      -> Vec<String>),
        ("license",       license       -> String),
        ("publish",       publish       -> manifest::VecStringOrBool),
        ("repository",    repository    -> String),
        ("rust-version",  rust_version  -> RustVersion),
        ("version",       version       -> semver::Version),
    }

    /// Gets a workspace dependency with the `name`.
    fn get_dependency(
        &self,
        name: &str,
        package_root: &Path,
    ) -> CargoResult<manifest::TomlDependency> {
        let Some(deps) = &self.dependencies else {
            bail!("`workspace.dependencies` was not defined");
        };
        let Some(dep) = deps.get(name) else {
            bail!("`dependency.{name}` was not found in `workspace.dependencies`");
        };
        let mut dep = dep.clone();
        if let manifest::TomlDependency::Detailed(detailed) = &mut dep {
            if let Some(rel_path) = &detailed.path {
                detailed.path = Some(resolve_relative_path(
                    name,
                    self.ws_root(),
                    package_root,
                    rel_path,
                )?);
            }
        }
        Ok(dep)
    }

    /// Gets the field `workspace.lint`.
    fn lints(&self) -> CargoResult<manifest::TomlLints> {
        let Some(val) = &self.lints else {
            bail!("`workspace.lints` was not defined");
        };
        Ok(val.clone())
    }

    /// Gets the field `workspace.package.license-file`.
    fn license_file(&self, package_root: &Path) -> CargoResult<String> {
        let Some(license_file) = self.package.as_ref().and_then(|p| p.license_file.as_ref()) else {
            bail!("`workspace.package.license-file` was not defined");
        };
        resolve_relative_path("license-file", &self._ws_root, package_root, license_file)
    }

    /// Gets the field `workspace.package.readme`.
    fn readme(&self, package_root: &Path) -> CargoResult<manifest::StringOrBool> {
        let Some(readme) = resolve_package_readme(
            self._ws_root.as_path(),
            self.package.as_ref().and_then(|p| p.readme.as_ref()),
        ) else {
            bail!("`workspace.package.readme` was not defined");
        };
        resolve_relative_path("readme", &self._ws_root, package_root, &readme)
            .map(manifest::StringOrBool::String)
    }

    fn ws_root(&self) -> &PathBuf {
        &self._ws_root
    }
}

fn field_inherit_with<'a, T>(
    field: manifest::InheritableField<T>,
    label: &str,
    get_ws_inheritable: impl FnOnce() -> CargoResult<T>,
) -> CargoResult<T> {
    match field {
        manifest::InheritableField::Value(value) => Ok(value),
        manifest::InheritableField::Inherit(_) => get_ws_inheritable().with_context(|| {
            format!(
                "error inheriting `{label}` from workspace root manifest's `workspace.package.{label}`",
            )
        }),
    }
}

fn lints_inherit_with(
    lints: manifest::InheritableLints,
    get_ws_inheritable: impl FnOnce() -> CargoResult<manifest::TomlLints>,
) -> CargoResult<manifest::TomlLints> {
    if lints.workspace {
        if !lints.lints.is_empty() {
            anyhow::bail!("cannot override `workspace.lints` in `lints`, either remove the overrides or `lints.workspace = true` and manually specify the lints");
        }
        get_ws_inheritable().with_context(|| {
            "error inheriting `lints` from workspace root manifest's `workspace.lints`"
        })
    } else {
        Ok(lints.lints)
    }
}

fn dependency_inherit_with<'a>(
    dependency: manifest::InheritableDependency,
    name: &str,
    inherit: &dyn Fn() -> CargoResult<&'a InheritableFields>,
    package_root: &Path,
    warnings: &mut Vec<String>,
) -> CargoResult<manifest::TomlDependency> {
    match dependency {
        manifest::InheritableDependency::Value(value) => Ok(value),
        manifest::InheritableDependency::Inherit(w) => {
            inner_dependency_inherit_with(w, name, inherit, package_root, warnings).with_context(|| {
                format!(
                    "error inheriting `{name}` from workspace root manifest's `workspace.dependencies.{name}`",
                )
            })
        }
    }
}

fn inner_dependency_inherit_with<'a>(
    dependency: manifest::TomlInheritedDependency,
    name: &str,
    inherit: &dyn Fn() -> CargoResult<&'a InheritableFields>,
    package_root: &Path,
    warnings: &mut Vec<String>,
) -> CargoResult<manifest::TomlDependency> {
    fn default_features_msg(label: &str, ws_def_feat: Option<bool>, warnings: &mut Vec<String>) {
        let ws_def_feat = match ws_def_feat {
            Some(true) => "true",
            Some(false) => "false",
            None => "not specified",
        };
        warnings.push(format!(
            "`default-features` is ignored for {label}, since `default-features` was \
                {ws_def_feat} for `workspace.dependencies.{label}`, \
                this could become a hard error in the future"
        ))
    }
    if dependency.default_features.is_some() && dependency.default_features2.is_some() {
        warn_on_deprecated("default-features", name, "dependency", warnings);
    }
    inherit()?.get_dependency(name, package_root).map(|d| {
        match d {
            manifest::TomlDependency::Simple(s) => {
                if let Some(false) = dependency.default_features() {
                    default_features_msg(name, None, warnings);
                }
                if dependency.optional.is_some()
                    || dependency.features.is_some()
                    || dependency.public.is_some()
                {
                    manifest::TomlDependency::Detailed(manifest::TomlDetailedDependency {
                        version: Some(s),
                        optional: dependency.optional,
                        features: dependency.features.clone(),
                        public: dependency.public,
                        ..Default::default()
                    })
                } else {
                    manifest::TomlDependency::Simple(s)
                }
            }
            manifest::TomlDependency::Detailed(d) => {
                let mut d = d.clone();
                match (dependency.default_features(), d.default_features()) {
                    // member: default-features = true and
                    // workspace: default-features = false should turn on
                    // default-features
                    (Some(true), Some(false)) => {
                        d.default_features = Some(true);
                    }
                    // member: default-features = false and
                    // workspace: default-features = true should ignore member
                    // default-features
                    (Some(false), Some(true)) => {
                        default_features_msg(name, Some(true), warnings);
                    }
                    // member: default-features = false and
                    // workspace: dep = "1.0" should ignore member default-features
                    (Some(false), None) => {
                        default_features_msg(name, None, warnings);
                    }
                    _ => {}
                }
                d.features = match (d.features.clone(), dependency.features.clone()) {
                    (Some(dep_feat), Some(inherit_feat)) => Some(
                        dep_feat
                            .into_iter()
                            .chain(inherit_feat)
                            .collect::<Vec<String>>(),
                    ),
                    (Some(dep_fet), None) => Some(dep_fet),
                    (None, Some(inherit_feat)) => Some(inherit_feat),
                    (None, None) => None,
                };
                d.optional = dependency.optional;
                manifest::TomlDependency::Detailed(d)
            }
        }
    })
}

#[tracing::instrument(skip_all)]
fn to_real_manifest(
    contents: String,
    document: toml_edit::ImDocument<String>,
    original_toml: manifest::TomlManifest,
    resolved_toml: manifest::TomlManifest,
    features: Features,
    workspace_config: WorkspaceConfig,
    source_id: SourceId,
    manifest_file: &Path,
    gctx: &GlobalContext,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> CargoResult<Manifest> {
    let embedded = is_embedded(manifest_file);
    let package_root = manifest_file.parent().unwrap();
    if !package_root.is_dir() {
        bail!(
            "package root '{}' is not a directory",
            package_root.display()
        );
    };

    let original_package = original_toml
        .package()
        .ok_or_else(|| anyhow::format_err!("no `package` section found"))?;

    let package_name = &original_package.name;
    if package_name.contains(':') {
        features.require(Feature::open_namespaces())?;
    }

    let resolved_package = resolved_toml
        .package()
        .expect("previously verified to have a `[package]`");
    let rust_version = resolved_package
        .resolved_rust_version()
        .expect("previously resolved")
        .cloned();

    let edition = if let Some(edition) = resolved_package
        .resolved_edition()
        .expect("previously resolved")
    {
        let edition: Edition = edition
            .parse()
            .with_context(|| "failed to parse the `edition` key")?;
        if let Some(pkg_msrv) = &rust_version {
            if let Some(edition_msrv) = edition.first_version() {
                let edition_msrv = RustVersion::try_from(edition_msrv).unwrap();
                if !edition_msrv.is_compatible_with(pkg_msrv.as_partial()) {
                    bail!(
                        "rust-version {} is older than first version ({}) required by \
                            the specified edition ({})",
                        pkg_msrv,
                        edition_msrv,
                        edition,
                    )
                }
            }
        }
        edition
    } else {
        let msrv_edition = if let Some(pkg_msrv) = &rust_version {
            Edition::ALL
                .iter()
                .filter(|e| {
                    e.first_version()
                        .map(|e| {
                            let e = RustVersion::try_from(e).unwrap();
                            e.is_compatible_with(pkg_msrv.as_partial())
                        })
                        .unwrap_or_default()
                })
                .max()
                .copied()
        } else {
            None
        }
        .unwrap_or_default();
        let default_edition = Edition::default();
        let latest_edition = Edition::LATEST_STABLE;

        // We're trying to help the user who might assume they are using a new edition,
        // so if they can't use a new edition, don't bother to tell them to set it.
        // This also avoids having to worry about whether `package.edition` is compatible with
        // their MSRV.
        if msrv_edition != default_edition {
            let tip = if msrv_edition == latest_edition {
                format!(" while the latest is {latest_edition}")
            } else {
                format!(" while {msrv_edition} is compatible with `rust-version`")
            };
            warnings.push(format!(
                "no edition set: defaulting to the {default_edition} edition{tip}",
            ));
        }
        default_edition
    };
    // Add these lines if start a new unstable edition.
    // ```
    // if edition == Edition::Edition20xx {
    //     features.require(Feature::edition20xx())?;
    // }
    // ```
    if edition == Edition::Edition2024 {
        features.require(Feature::edition2024())?;
    } else if !edition.is_stable() {
        // Guard in case someone forgets to add .require()
        return Err(util::errors::internal(format!(
            "edition {} should be gated",
            edition
        )));
    }

    if original_toml.project.is_some() {
        if Edition::Edition2024 <= edition {
            anyhow::bail!(
                "`[project]` is not supported as of the 2024 Edition, please use `[package]`"
            );
        } else {
            warnings.push(format!("`[project]` is deprecated in favor of `[package]`"));
        }
    }

    if resolved_package.metabuild.is_some() {
        features.require(Feature::metabuild())?;
    }

    let resolve_behavior = match (
        resolved_package.resolver.as_ref(),
        resolved_toml
            .workspace
            .as_ref()
            .and_then(|ws| ws.resolver.as_ref()),
    ) {
        (None, None) => None,
        (Some(s), None) | (None, Some(s)) => Some(ResolveBehavior::from_manifest(s)?),
        (Some(_), Some(_)) => {
            bail!("cannot specify `resolver` field in both `[workspace]` and `[package]`")
        }
    };

    // If we have no lib at all, use the inferred lib, if available.
    // If we have a lib with a path, we're done.
    // If we have a lib with no path, use the inferred lib or else the package name.
    let targets = to_targets(
        &features,
        &resolved_toml,
        package_name,
        package_root,
        edition,
        &resolved_package.build,
        &resolved_package.metabuild,
        warnings,
        errors,
    )?;

    if targets.iter().all(|t| t.is_custom_build()) {
        bail!(
            "no targets specified in the manifest\n\
                 either src/lib.rs, src/main.rs, a [lib] section, or \
                 [[bin]] section must be present"
        )
    }

    if let Err(conflict_targets) = unique_build_targets(&targets, package_root) {
        conflict_targets
            .iter()
            .for_each(|(target_path, conflicts)| {
                warnings.push(format!(
                    "file `{}` found to be present in multiple \
                 build targets:\n{}",
                    target_path.display().to_string(),
                    conflicts
                        .iter()
                        .map(|t| format!("  * `{}` target `{}`", t.kind().description(), t.name(),))
                        .join("\n")
                ));
            })
    }

    if let Some(links) = &resolved_package.links {
        if !targets.iter().any(|t| t.is_custom_build()) {
            bail!("package specifies that it links to `{links}` but does not have a custom build script")
        }
    }

    validate_dependencies(original_toml.dependencies.as_ref(), None, None, warnings)?;
    if original_toml.dev_dependencies.is_some() && original_toml.dev_dependencies2.is_some() {
        warn_on_deprecated("dev-dependencies", package_name, "package", warnings);
    }
    validate_dependencies(
        original_toml.dev_dependencies(),
        None,
        Some(DepKind::Development),
        warnings,
    )?;
    if original_toml.build_dependencies.is_some() && original_toml.build_dependencies2.is_some() {
        warn_on_deprecated("build-dependencies", package_name, "package", warnings);
    }
    validate_dependencies(
        original_toml.build_dependencies(),
        None,
        Some(DepKind::Build),
        warnings,
    )?;
    for (name, platform) in original_toml.target.iter().flatten() {
        let platform_kind: Platform = name.parse()?;
        platform_kind.check_cfg_attributes(warnings);
        let platform_kind = Some(platform_kind);
        validate_dependencies(
            platform.dependencies.as_ref(),
            platform_kind.as_ref(),
            None,
            warnings,
        )?;
        if platform.build_dependencies.is_some() && platform.build_dependencies2.is_some() {
            warn_on_deprecated("build-dependencies", name, "platform target", warnings);
        }
        validate_dependencies(
            platform.build_dependencies(),
            platform_kind.as_ref(),
            Some(DepKind::Build),
            warnings,
        )?;
        if platform.dev_dependencies.is_some() && platform.dev_dependencies2.is_some() {
            warn_on_deprecated("dev-dependencies", name, "platform target", warnings);
        }
        validate_dependencies(
            platform.dev_dependencies(),
            platform_kind.as_ref(),
            Some(DepKind::Development),
            warnings,
        )?;
    }

    // Collect the dependencies.
    let mut deps = Vec::new();
    let mut manifest_ctx = ManifestContext {
        deps: &mut deps,
        source_id,
        gctx,
        warnings,
        platform: None,
        root: package_root,
    };
    gather_dependencies(&mut manifest_ctx, resolved_toml.dependencies.as_ref(), None)?;
    gather_dependencies(
        &mut manifest_ctx,
        resolved_toml.dev_dependencies(),
        Some(DepKind::Development),
    )?;
    gather_dependencies(
        &mut manifest_ctx,
        resolved_toml.build_dependencies(),
        Some(DepKind::Build),
    )?;
    for (name, platform) in resolved_toml.target.iter().flatten() {
        manifest_ctx.platform = Some(name.parse()?);
        gather_dependencies(&mut manifest_ctx, platform.dependencies.as_ref(), None)?;
        gather_dependencies(
            &mut manifest_ctx,
            platform.build_dependencies(),
            Some(DepKind::Build),
        )?;
        gather_dependencies(
            &mut manifest_ctx,
            platform.dev_dependencies(),
            Some(DepKind::Development),
        )?;
    }
    let replace = replace(&resolved_toml, &mut manifest_ctx)?;
    let patch = patch(&resolved_toml, &mut manifest_ctx, &features)?;

    {
        let mut names_sources = BTreeMap::new();
        for dep in &deps {
            let name = dep.name_in_toml();
            let prev = names_sources.insert(name, dep.source_id());
            if prev.is_some() && prev != Some(dep.source_id()) {
                bail!(
                    "Dependency '{}' has different source paths depending on the build \
                         target. Each dependency must have a single canonical source path \
                         irrespective of build target.",
                    name
                );
            }
        }
    }

    verify_lints(
        resolved_toml.resolved_lints().expect("previously resolved"),
        gctx,
        warnings,
    )?;
    let default = manifest::TomlLints::default();
    let rustflags = lints_to_rustflags(
        resolved_toml
            .resolved_lints()
            .expect("previously resolved")
            .unwrap_or(&default),
    );

    let metadata = ManifestMetadata {
        description: resolved_package
            .resolved_description()
            .expect("previously resolved")
            .cloned(),
        homepage: resolved_package
            .resolved_homepage()
            .expect("previously resolved")
            .cloned(),
        documentation: resolved_package
            .resolved_documentation()
            .expect("previously resolved")
            .cloned(),
        readme: resolved_package
            .resolved_readme()
            .expect("previously resolved")
            .cloned(),
        authors: resolved_package
            .resolved_authors()
            .expect("previously resolved")
            .cloned()
            .unwrap_or_default(),
        license: resolved_package
            .resolved_license()
            .expect("previously resolved")
            .cloned(),
        license_file: resolved_package
            .resolved_license_file()
            .expect("previously resolved")
            .cloned(),
        repository: resolved_package
            .resolved_repository()
            .expect("previously resolved")
            .cloned(),
        keywords: resolved_package
            .resolved_keywords()
            .expect("previously resolved")
            .cloned()
            .unwrap_or_default(),
        categories: resolved_package
            .resolved_categories()
            .expect("previously resolved")
            .cloned()
            .unwrap_or_default(),
        badges: resolved_toml
            .resolved_badges()
            .expect("previously resolved")
            .cloned()
            .unwrap_or_default(),
        links: resolved_package.links.clone(),
        rust_version: rust_version.clone(),
    };

    if let Some(profiles) = &resolved_toml.profile {
        let cli_unstable = gctx.cli_unstable();
        validate_profiles(profiles, cli_unstable, &features, warnings)?;
    }

    let version = resolved_package
        .resolved_version()
        .expect("previously resolved");
    let publish = match resolved_package
        .resolved_publish()
        .expect("previously resolved")
    {
        Some(manifest::VecStringOrBool::VecString(ref vecstring)) => Some(vecstring.clone()),
        Some(manifest::VecStringOrBool::Bool(false)) => Some(vec![]),
        Some(manifest::VecStringOrBool::Bool(true)) => None,
        None => version.is_none().then_some(vec![]),
    };

    if version.is_none() && publish != Some(vec![]) {
        bail!("`package.publish` requires `package.version` be specified");
    }

    let pkgid = PackageId::new(
        resolved_package.name.as_str().into(),
        version
            .cloned()
            .unwrap_or_else(|| semver::Version::new(0, 0, 0)),
        source_id,
    );
    let summary = Summary::new(
        pkgid,
        deps,
        &resolved_toml
            .features
            .as_ref()
            .unwrap_or(&Default::default())
            .iter()
            .map(|(k, v)| {
                (
                    InternedString::new(k),
                    v.iter().map(InternedString::from).collect(),
                )
            })
            .collect(),
        resolved_package.links.as_deref(),
        rust_version.clone(),
    )?;
    if summary.features().contains_key("default-features") {
        warnings.push(
            "`default-features = [\"..\"]` was found in [features]. \
                 Did you mean to use `default = [\"..\"]`?"
                .to_string(),
        )
    }

    if let Some(run) = &resolved_package.default_run {
        if !targets
            .iter()
            .filter(|t| t.is_bin())
            .any(|t| t.name() == run)
        {
            let suggestion =
                util::closest_msg(run, targets.iter().filter(|t| t.is_bin()), |t| t.name());
            bail!("default-run target `{}` not found{}", run, suggestion);
        }
    }

    let default_kind = resolved_package
        .default_target
        .as_ref()
        .map(|t| CompileTarget::new(&*t))
        .transpose()?
        .map(CompileKind::Target);
    let forced_kind = resolved_package
        .forced_target
        .as_ref()
        .map(|t| CompileTarget::new(&*t))
        .transpose()?
        .map(CompileKind::Target);
    let include = resolved_package
        .resolved_include()
        .expect("previously resolved")
        .cloned()
        .unwrap_or_default();
    let exclude = resolved_package
        .resolved_exclude()
        .expect("previously resolved")
        .cloned()
        .unwrap_or_default();
    let links = resolved_package.links.clone();
    let custom_metadata = resolved_package.metadata.clone();
    let im_a_teapot = resolved_package.im_a_teapot;
    let default_run = resolved_package.default_run.clone();
    let metabuild = resolved_package.metabuild.clone().map(|sov| sov.0);
    let manifest = Manifest::new(
        Rc::new(contents),
        Rc::new(document),
        Rc::new(original_toml),
        Rc::new(resolved_toml),
        summary,
        default_kind,
        forced_kind,
        targets,
        exclude,
        include,
        links,
        metadata,
        custom_metadata,
        publish,
        replace,
        patch,
        workspace_config,
        features,
        edition,
        rust_version,
        im_a_teapot,
        default_run,
        metabuild,
        resolve_behavior,
        rustflags,
        embedded,
    );
    if manifest
        .resolved_toml()
        .package()
        .unwrap()
        .license_file
        .is_some()
        && manifest
            .resolved_toml()
            .package()
            .unwrap()
            .license
            .is_some()
    {
        warnings.push(
            "only one of `license` or `license-file` is necessary\n\
                 `license` should be used if the package license can be expressed \
                 with a standard SPDX expression.\n\
                 `license-file` should be used if the package uses a non-standard license.\n\
                 See https://doc.rust-lang.org/cargo/reference/manifest.html#the-license-and-license-file-fields \
                 for more information."
                .to_owned(),
        );
    }
    warn_on_unused(&manifest.original_toml()._unused_keys, warnings);

    manifest.feature_gate()?;

    Ok(manifest)
}

fn to_virtual_manifest(
    contents: String,
    document: toml_edit::ImDocument<String>,
    original_toml: manifest::TomlManifest,
    resolved_toml: manifest::TomlManifest,
    features: Features,
    workspace_config: WorkspaceConfig,
    source_id: SourceId,
    manifest_file: &Path,
    gctx: &GlobalContext,
    warnings: &mut Vec<String>,
    _errors: &mut Vec<String>,
) -> CargoResult<VirtualManifest> {
    let root = manifest_file.parent().unwrap();

    let mut deps = Vec::new();
    let (replace, patch) = {
        let mut manifest_ctx = ManifestContext {
            deps: &mut deps,
            source_id,
            gctx,
            warnings,
            platform: None,
            root,
        };
        (
            replace(&original_toml, &mut manifest_ctx)?,
            patch(&original_toml, &mut manifest_ctx, &features)?,
        )
    };
    if let Some(profiles) = &original_toml.profile {
        validate_profiles(profiles, gctx.cli_unstable(), &features, warnings)?;
    }
    let resolve_behavior = original_toml
        .workspace
        .as_ref()
        .and_then(|ws| ws.resolver.as_deref())
        .map(|r| ResolveBehavior::from_manifest(r))
        .transpose()?;
    if let WorkspaceConfig::Member { .. } = &workspace_config {
        bail!("virtual manifests must be configured with [workspace]");
    }
    let manifest = VirtualManifest::new(
        Rc::new(contents),
        Rc::new(document),
        Rc::new(original_toml),
        Rc::new(resolved_toml),
        replace,
        patch,
        workspace_config,
        features,
        resolve_behavior,
    );

    warn_on_unused(&manifest.original_toml()._unused_keys, warnings);

    Ok(manifest)
}

#[tracing::instrument(skip_all)]
fn validate_dependencies(
    original_deps: Option<&BTreeMap<manifest::PackageName, manifest::InheritableDependency>>,
    platform: Option<&Platform>,
    kind: Option<DepKind>,
    warnings: &mut Vec<String>,
) -> CargoResult<()> {
    let Some(dependencies) = original_deps else {
        return Ok(());
    };

    for (name_in_toml, v) in dependencies.iter() {
        let kind_name = match kind {
            Some(k) => k.kind_table(),
            None => "dependencies",
        };
        let table_in_toml = if let Some(platform) = platform {
            format!("target.{}.{kind_name}", platform.to_string())
        } else {
            kind_name.to_string()
        };
        unused_dep_keys(name_in_toml, &table_in_toml, v.unused_keys(), warnings);
    }
    Ok(())
}

struct ManifestContext<'a, 'b> {
    deps: &'a mut Vec<Dependency>,
    source_id: SourceId,
    gctx: &'b GlobalContext,
    warnings: &'a mut Vec<String>,
    platform: Option<Platform>,
    root: &'a Path,
}

#[tracing::instrument(skip_all)]
fn gather_dependencies(
    manifest_ctx: &mut ManifestContext<'_, '_>,
    resolved_deps: Option<&BTreeMap<manifest::PackageName, manifest::InheritableDependency>>,
    kind: Option<DepKind>,
) -> CargoResult<()> {
    let Some(dependencies) = resolved_deps else {
        return Ok(());
    };

    for (n, v) in dependencies.iter() {
        let resolved = v.resolved().expect("previously resolved");
        let dep = dep_to_dependency(&resolved, n, manifest_ctx, kind, None)?;
        manifest_ctx.deps.push(dep);
    }
    Ok(())
}

fn replace(
    me: &manifest::TomlManifest,
    manifest_ctx: &mut ManifestContext<'_, '_>,
) -> CargoResult<Vec<(PackageIdSpec, Dependency)>> {
    if me.patch.is_some() && me.replace.is_some() {
        bail!("cannot specify both [replace] and [patch]");
    }
    let mut replace = Vec::new();
    for (spec, replacement) in me.replace.iter().flatten() {
        let mut spec = PackageIdSpec::parse(spec).with_context(|| {
            format!(
                "replacements must specify a valid semver \
                     version to replace, but `{}` does not",
                spec
            )
        })?;
        if spec.url().is_none() {
            spec.set_url(CRATES_IO_INDEX.parse().unwrap());
        }

        if replacement.is_version_specified() {
            bail!(
                "replacements cannot specify a version \
                     requirement, but found one for `{}`",
                spec
            );
        }

        let mut dep = dep_to_dependency(replacement, spec.name(), manifest_ctx, None, None)?;
        let version = spec.version().ok_or_else(|| {
            anyhow!(
                "replacements must specify a version \
                     to replace, but `{}` does not",
                spec
            )
        })?;
        unused_dep_keys(
            dep.name_in_toml().as_str(),
            "replace",
            replacement.unused_keys(),
            &mut manifest_ctx.warnings,
        );
        dep.set_version_req(OptVersionReq::exact(&version));
        replace.push((spec, dep));
    }
    Ok(replace)
}

fn patch(
    me: &manifest::TomlManifest,
    manifest_ctx: &mut ManifestContext<'_, '_>,
    features: &Features,
) -> CargoResult<HashMap<Url, Vec<Dependency>>> {
    let patch_files_enabled = features.require(Feature::patch_files()).is_ok();
    let mut patch = HashMap::new();
    for (toml_url, deps) in me.patch.iter().flatten() {
        let url = match &toml_url[..] {
            CRATES_IO_REGISTRY => CRATES_IO_INDEX.parse().unwrap(),
            _ => manifest_ctx
                .gctx
                .get_registry_index(toml_url)
                .or_else(|_| toml_url.into_url())
                .with_context(|| {
                    format!(
                        "[patch] entry `{}` should be a URL or registry name",
                        toml_url
                    )
                })?,
        };
        patch.insert(
            url.clone(),
            deps.iter()
                .map(|(name, dep)| {
                    unused_dep_keys(
                        name,
                        &format!("patch.{toml_url}",),
                        dep.unused_keys(),
                        &mut manifest_ctx.warnings,
                    );
                    dep_to_dependency(
                        dep,
                        name,
                        manifest_ctx,
                        None,
                        Some((&url, patch_files_enabled)),
                    )
                })
                .collect::<CargoResult<Vec<_>>>()?,
        );
    }
    Ok(patch)
}

/// Transforms a `patch` entry to a [`Dependency`].
pub(crate) fn to_dependency<P: ResolveToPath + Clone>(
    dep: &manifest::TomlDependency<P>,
    name: &str,
    source_id: SourceId,
    gctx: &GlobalContext,
    warnings: &mut Vec<String>,
    platform: Option<Platform>,
    root: &Path,
    kind: Option<DepKind>,
    patch_source_url: &Url,
) -> CargoResult<Dependency> {
    let manifest_ctx = &mut ManifestContext {
        deps: &mut Vec::new(),
        source_id,
        gctx,
        warnings,
        platform,
        root,
    };
    let patch_source_url = Some((patch_source_url, gctx.cli_unstable().patch_files));
    dep_to_dependency(dep, name, manifest_ctx, kind, patch_source_url)
}

fn dep_to_dependency<P: ResolveToPath + Clone>(
    orig: &manifest::TomlDependency<P>,
    name: &str,
    manifest_ctx: &mut ManifestContext<'_, '_>,
    kind: Option<DepKind>,
    patch_source_url: Option<(&Url, bool)>,
) -> CargoResult<Dependency> {
    match *orig {
        manifest::TomlDependency::Simple(ref version) => detailed_dep_to_dependency(
            &manifest::TomlDetailedDependency::<P> {
                version: Some(version.clone()),
                ..Default::default()
            },
            name,
            manifest_ctx,
            kind,
            patch_source_url,
        ),
        manifest::TomlDependency::Detailed(ref details) => {
            detailed_dep_to_dependency(details, name, manifest_ctx, kind, patch_source_url)
        }
    }
}

fn detailed_dep_to_dependency<P: ResolveToPath + Clone>(
    orig: &manifest::TomlDetailedDependency<P>,
    name_in_toml: &str,
    manifest_ctx: &mut ManifestContext<'_, '_>,
    kind: Option<DepKind>,
    patch_source_url: Option<(&Url, bool)>,
) -> CargoResult<Dependency> {
    if orig.version.is_none() && orig.path.is_none() && orig.git.is_none() {
        let msg = format!(
            "dependency ({}) specified without \
                 providing a local path, Git repository, version, or \
                 workspace dependency to use. This will be considered an \
                 error in future versions",
            name_in_toml
        );
        manifest_ctx.warnings.push(msg);
    }

    if let Some(version) = &orig.version {
        if version.contains('+') {
            manifest_ctx.warnings.push(format!(
                "version requirement `{}` for dependency `{}` \
                     includes semver metadata which will be ignored, removing the \
                     metadata is recommended to avoid confusion",
                version, name_in_toml
            ));
        }
    }

    if orig.git.is_none() {
        let git_only_keys = [
            (&orig.branch, "branch"),
            (&orig.tag, "tag"),
            (&orig.rev, "rev"),
        ];

        for &(key, key_name) in &git_only_keys {
            if key.is_some() {
                bail!(
                    "key `{}` is ignored for dependency ({}).",
                    key_name,
                    name_in_toml
                );
            }
        }
    }

    // Early detection of potentially misused feature syntax
    // instead of generating a "feature not found" error.
    if let Some(features) = &orig.features {
        for feature in features {
            if feature.contains('/') {
                bail!(
                    "feature `{}` in dependency `{}` is not allowed to contain slashes\n\
                         If you want to enable features of a transitive dependency, \
                         the direct dependency needs to re-export those features from \
                         the `[features]` table.",
                    feature,
                    name_in_toml
                );
            }
            if feature.starts_with("dep:") {
                bail!(
                    "feature `{}` in dependency `{}` is not allowed to use explicit \
                        `dep:` syntax\n\
                         If you want to enable an optional dependency, specify the name \
                         of the optional dependency without the `dep:` prefix, or specify \
                         a feature from the dependency's `[features]` table that enables \
                         the optional dependency.",
                    feature,
                    name_in_toml
                );
            }
        }
    }

    let new_source_id = resolve_source_id_from_dependency(orig, name_in_toml, manifest_ctx)?;

    let (pkg_name, explicit_name_in_toml) = match orig.package {
        Some(ref s) => (&s[..], Some(name_in_toml)),
        None => (name_in_toml, None),
    };

    let version = orig.version.as_deref();
    let mut dep = Dependency::parse(pkg_name, version, new_source_id)?;

    if orig.default_features.is_some() && orig.default_features2.is_some() {
        warn_on_deprecated(
            "default-features",
            name_in_toml,
            "dependency",
            manifest_ctx.warnings,
        );
    }
    dep.set_features(orig.features.iter().flatten())
        .set_default_features(orig.default_features().unwrap_or(true))
        .set_optional(orig.optional.unwrap_or(false))
        .set_platform(manifest_ctx.platform.clone());
    if let Some(registry) = &orig.registry {
        let registry_id = SourceId::alt_registry(manifest_ctx.gctx, registry)?;
        dep.set_registry_id(registry_id);
    }
    if let Some(registry_index) = &orig.registry_index {
        let url = registry_index.into_url()?;
        let registry_id = SourceId::for_registry(&url)?;
        dep.set_registry_id(registry_id);
    }

    if let Some(kind) = kind {
        dep.set_kind(kind);
    }
    if let Some(name_in_toml) = explicit_name_in_toml {
        dep.set_explicit_name_in_toml(name_in_toml);
    }

    if let Some(p) = orig.public {
        dep.set_public(p);
    }

    if let (Some(artifact), is_lib, target) = (
        orig.artifact.as_ref(),
        orig.lib.unwrap_or(false),
        orig.target.as_deref(),
    ) {
        if manifest_ctx.gctx.cli_unstable().bindeps {
            let artifact = Artifact::parse(&artifact.0, is_lib, target)?;
            if dep.kind() != DepKind::Build
                && artifact.target() == Some(ArtifactTarget::BuildDependencyAssumeTarget)
            {
                bail!(
                    r#"`target = "target"` in normal- or dev-dependencies has no effect ({})"#,
                    name_in_toml
                );
            }
            dep.set_artifact(artifact)
        } else {
            bail!("`artifact = …` requires `-Z bindeps` ({})", name_in_toml);
        }
    } else if orig.lib.is_some() || orig.target.is_some() {
        for (is_set, specifier) in [
            (orig.lib.is_some(), "lib"),
            (orig.target.is_some(), "target"),
        ] {
            if !is_set {
                continue;
            }
            bail!(
                "'{}' specifier cannot be used without an 'artifact = …' value ({})",
                specifier,
                name_in_toml
            )
        }
    }

    if let Some(source_id) = patched_source_id(orig, manifest_ctx, &dep, patch_source_url)? {
        dep.set_source_id(source_id);
    }

    Ok(dep)
}

fn resolve_source_id_from_dependency<P: ResolveToPath + Clone>(
    orig: &manifest::TomlDetailedDependency<P>,
    name_in_toml: &str,
    manifest_ctx: &mut ManifestContext<'_, '_>,
) -> CargoResult<SourceId> {
    let new_source_id = match (
        orig.git.as_ref(),
        orig.path.as_ref(),
        orig.registry.as_ref(),
        orig.registry_index.as_ref(),
    ) {
        (Some(_), _, Some(_), _) | (Some(_), _, _, Some(_)) => bail!(
            "dependency ({}) specification is ambiguous. \
                 Only one of `git` or `registry` is allowed.",
            name_in_toml
        ),
        (_, _, Some(_), Some(_)) => bail!(
            "dependency ({}) specification is ambiguous. \
                 Only one of `registry` or `registry-index` is allowed.",
            name_in_toml
        ),
        (Some(git), maybe_path, _, _) => {
            if maybe_path.is_some() {
                bail!(
                    "dependency ({}) specification is ambiguous. \
                         Only one of `git` or `path` is allowed.",
                    name_in_toml
                );
            }

            let n_details = [&orig.branch, &orig.tag, &orig.rev]
                .iter()
                .filter(|d| d.is_some())
                .count();

            if n_details > 1 {
                bail!(
                    "dependency ({}) specification is ambiguous. \
                         Only one of `branch`, `tag` or `rev` is allowed.",
                    name_in_toml
                );
            }

            let reference = orig
                .branch
                .clone()
                .map(GitReference::Branch)
                .or_else(|| orig.tag.clone().map(GitReference::Tag))
                .or_else(|| orig.rev.clone().map(GitReference::Rev))
                .unwrap_or(GitReference::DefaultBranch);
            let loc = git.into_url()?;

            if let Some(fragment) = loc.fragment() {
                let msg = format!(
                    "URL fragment `#{}` in git URL is ignored for dependency ({}). \
                        If you were trying to specify a specific git revision, \
                        use `rev = \"{}\"` in the dependency declaration.",
                    fragment, name_in_toml, fragment
                );
                manifest_ctx.warnings.push(msg)
            }

            SourceId::for_git(&loc, reference)?
        }
        (None, Some(path), _, _) => {
            let path = path.resolve(manifest_ctx.gctx);
            // If the source ID for the package we're parsing is a path
            // source, then we normalize the path here to get rid of
            // components like `..`.
            //
            // The purpose of this is to get a canonical ID for the package
            // that we're depending on to ensure that builds of this package
            // always end up hashing to the same value no matter where it's
            // built from.
            if manifest_ctx.source_id.is_path() {
                let path = manifest_ctx.root.join(path);
                let path = paths::normalize_path(&path);
                SourceId::for_path(&path)?
            } else {
                manifest_ctx.source_id
            }
        }
        (None, None, Some(registry), None) => SourceId::alt_registry(manifest_ctx.gctx, registry)?,
        (None, None, None, Some(registry_index)) => {
            let url = registry_index.into_url()?;
            SourceId::for_registry(&url)?
        }
        (None, None, None, None) => SourceId::crates_io(manifest_ctx.gctx)?,
    };

    Ok(new_source_id)
}

// Handle `patches` field for `[patch]` table, if any.
fn patched_source_id<P: ResolveToPath + Clone>(
    orig: &manifest::TomlDetailedDependency<P>,
    manifest_ctx: &mut ManifestContext<'_, '_>,
    dep: &Dependency,
    patch_source_url: Option<(&Url, bool)>,
) -> CargoResult<Option<SourceId>> {
    let name_in_toml = dep.name_in_toml().as_str();
    let message = "see https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#patch-files about the status of this feature.";
    match (patch_source_url, orig.patches.as_ref()) {
        (_, None) => {
            // not a SourceKind::Patched dep.
            Ok(None)
        }
        (None, Some(_)) => {
            let kind = dep.kind().kind_table();
            manifest_ctx.warnings.push(format!(
                "unused manifest key: {kind}.{name_in_toml}.patches; {message}"
            ));
            Ok(None)
        }
        (Some((url, false)), Some(_)) => {
            manifest_ctx.warnings.push(format!(
                "ignoring `patches` on patch for `{name_in_toml}` in `{url}`; {message}"
            ));
            Ok(None)
        }
        (Some((url, true)), Some(patches)) => {
            let source_id = dep.source_id();
            if !source_id.is_registry() {
                bail!(
                    "patch for `{name_in_toml}` in `{url}` requires a registry source \
                    when patching with patch files"
                );
            }
            if &CanonicalUrl::new(url)? != source_id.canonical_url() {
                bail!(
                    "patch for `{name_in_toml}` in `{url}` must refer to the same source \
                    when patching with patch files"
                )
            }
            let version = match dep.version_req().locked_version() {
                Some(v) => Some(v.to_owned()),
                None if dep.version_req().is_exact() => {
                    // Remove the `=` exact operator.
                    orig.version
                        .as_deref()
                        .map(|v| v[1..].trim().parse().ok())
                        .flatten()
                }
                None => None,
            };
            let Some(version) = version else {
                bail!(
                    "patch for `{name_in_toml}` in `{url}` requires an exact version \
                    when patching with patch files"
                );
            };
            let patches: Vec<_> = patches
                .iter()
                .map(|path| {
                    let path = path.resolve(manifest_ctx.gctx);
                    let path = manifest_ctx.root.join(path);
                    // keep paths inside workspace relative to workspace, otherwise absolute.
                    path.strip_prefix(manifest_ctx.gctx.cwd())
                        .map(Into::into)
                        .unwrap_or_else(|_| paths::normalize_path(&path))
                })
                .collect();
            if patches.is_empty() {
                bail!(
                    "patch for `{name_in_toml}` in `{url}` requires at least one patch file \
                    when patching with patch files"
                );
            }
            let pkg_name = dep.package_name().to_string();
            let patch_info = PatchInfo::new(pkg_name, version.to_string(), patches);
            SourceId::for_patches(source_id, patch_info).map(Some)
        }
    }
}

pub trait ResolveToPath {
    fn resolve(&self, gctx: &GlobalContext) -> PathBuf;
}

impl ResolveToPath for String {
    fn resolve(&self, _: &GlobalContext) -> PathBuf {
        self.into()
    }
}

impl ResolveToPath for ConfigRelativePath {
    fn resolve(&self, gctx: &GlobalContext) -> PathBuf {
        self.resolve_path(gctx)
    }
}

/// Checks a list of build targets, and ensures the target names are unique within a vector.
/// If not, the name of the offending build target is returned.
#[tracing::instrument(skip_all)]
fn unique_build_targets(
    targets: &[Target],
    package_root: &Path,
) -> Result<(), HashMap<PathBuf, Vec<Target>>> {
    let mut source_targets = HashMap::<_, Vec<_>>::new();
    for target in targets {
        if let TargetSourcePath::Path(path) = target.src_path() {
            let full = package_root.join(path);
            source_targets.entry(full).or_default().push(target.clone());
        }
    }

    let conflict_targets = source_targets
        .into_iter()
        .filter(|(_, targets)| targets.len() > 1)
        .collect::<HashMap<_, _>>();

    if !conflict_targets.is_empty() {
        return Err(conflict_targets);
    }

    Ok(())
}

/// Checks syntax validity and unstable feature gate for each profile.
///
/// It's a bit unfortunate both `-Z` flags and `cargo-features` are required,
/// because profiles can now be set in either `Cargo.toml` or `config.toml`.
fn validate_profiles(
    profiles: &manifest::TomlProfiles,
    cli_unstable: &CliUnstable,
    features: &Features,
    warnings: &mut Vec<String>,
) -> CargoResult<()> {
    for (name, profile) in &profiles.0 {
        validate_profile(profile, name, cli_unstable, features, warnings)?;
    }
    Ok(())
}

/// Checks stytax validity and unstable feature gate for a given profile.
pub fn validate_profile(
    root: &manifest::TomlProfile,
    name: &str,
    cli_unstable: &CliUnstable,
    features: &Features,
    warnings: &mut Vec<String>,
) -> CargoResult<()> {
    validate_profile_layer(root, name, cli_unstable, features)?;
    if let Some(ref profile) = root.build_override {
        validate_profile_override(profile, "build-override")?;
        validate_profile_layer(
            profile,
            &format!("{name}.build-override"),
            cli_unstable,
            features,
        )?;
    }
    if let Some(ref packages) = root.package {
        for (override_name, profile) in packages {
            validate_profile_override(profile, "package")?;
            validate_profile_layer(
                profile,
                &format!("{name}.package.{override_name}"),
                cli_unstable,
                features,
            )?;
        }
    }

    if let Some(dir_name) = &root.dir_name {
        // This is disabled for now, as we would like to stabilize named
        // profiles without this, and then decide in the future if it is
        // needed. This helps simplify the UI a little.
        bail!(
            "dir-name=\"{}\" in profile `{}` is not currently allowed, \
                 directory names are tied to the profile name for custom profiles",
            dir_name,
            name
        );
    }

    // `inherits` validation
    if matches!(root.inherits.as_deref(), Some("debug")) {
        bail!(
            "profile.{}.inherits=\"debug\" should be profile.{}.inherits=\"dev\"",
            name,
            name
        );
    }

    match name {
        "doc" => {
            warnings.push("profile `doc` is deprecated and has no effect".to_string());
        }
        "test" | "bench" => {
            if root.panic.is_some() {
                warnings.push(format!("`panic` setting is ignored for `{}` profile", name))
            }
        }
        _ => {}
    }

    if let Some(panic) = &root.panic {
        if panic != "unwind" && panic != "abort" {
            bail!(
                "`panic` setting of `{}` is not a valid setting, \
                     must be `unwind` or `abort`",
                panic
            );
        }
    }

    if let Some(manifest::StringOrBool::String(arg)) = &root.lto {
        if arg == "true" || arg == "false" {
            bail!(
                "`lto` setting of string `\"{arg}\"` for `{name}` profile is not \
                     a valid setting, must be a boolean (`true`/`false`) or a string \
                    (`\"thin\"`/`\"fat\"`/`\"off\"`) or omitted.",
            );
        }
    }

    Ok(())
}

/// Validates a profile.
///
/// This is a shallow check, which is reused for the profile itself and any overrides.
fn validate_profile_layer(
    profile: &manifest::TomlProfile,
    name: &str,
    cli_unstable: &CliUnstable,
    features: &Features,
) -> CargoResult<()> {
    if let Some(codegen_backend) = &profile.codegen_backend {
        match (
            features.require(Feature::codegen_backend()),
            cli_unstable.codegen_backend,
        ) {
            (Err(e), false) => return Err(e),
            _ => {}
        }

        if codegen_backend.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            bail!(
                "`profile.{}.codegen-backend` setting of `{}` is not a valid backend name.",
                name,
                codegen_backend,
            );
        }
    }
    if profile.rustflags.is_some() {
        match (
            features.require(Feature::profile_rustflags()),
            cli_unstable.profile_rustflags,
        ) {
            (Err(e), false) => return Err(e),
            _ => {}
        }
    }
    if profile.trim_paths.is_some() {
        match (
            features.require(Feature::trim_paths()),
            cli_unstable.trim_paths,
        ) {
            (Err(e), false) => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Validation that is specific to an override.
fn validate_profile_override(profile: &manifest::TomlProfile, which: &str) -> CargoResult<()> {
    if profile.package.is_some() {
        bail!("package-specific profiles cannot be nested");
    }
    if profile.build_override.is_some() {
        bail!("build-override profiles cannot be nested");
    }
    if profile.panic.is_some() {
        bail!("`panic` may not be specified in a `{}` profile", which)
    }
    if profile.lto.is_some() {
        bail!("`lto` may not be specified in a `{}` profile", which)
    }
    if profile.rpath.is_some() {
        bail!("`rpath` may not be specified in a `{}` profile", which)
    }
    Ok(())
}

fn verify_lints(
    lints: Option<&manifest::TomlLints>,
    gctx: &GlobalContext,
    warnings: &mut Vec<String>,
) -> CargoResult<()> {
    let Some(lints) = lints else {
        return Ok(());
    };

    for (tool, lints) in lints {
        let supported = ["cargo", "clippy", "rust", "rustdoc"];
        if !supported.contains(&tool.as_str()) {
            let supported = supported.join(", ");
            anyhow::bail!("unsupported `{tool}` in `[lints]`, must be one of {supported}")
        }
        if tool == "cargo" && !gctx.cli_unstable().cargo_lints {
            warn_for_cargo_lint_feature(gctx, warnings);
        }
        for name in lints.keys() {
            if let Some((prefix, suffix)) = name.split_once("::") {
                if tool == prefix {
                    anyhow::bail!(
                        "`lints.{tool}.{name}` is not valid lint name; try `lints.{prefix}.{suffix}`"
                    )
                } else if tool == "rust" && supported.contains(&prefix) {
                    anyhow::bail!(
                        "`lints.{tool}.{name}` is not valid lint name; try `lints.{prefix}.{suffix}`"
                    )
                } else {
                    anyhow::bail!("`lints.{tool}.{name}` is not a valid lint name")
                }
            }
        }
    }

    Ok(())
}

fn warn_for_cargo_lint_feature(gctx: &GlobalContext, warnings: &mut Vec<String>) {
    use std::fmt::Write as _;

    let key_name = "lints.cargo";
    let feature_name = "cargo-lints";

    let mut message = String::new();

    let _ = write!(
        message,
        "unused manifest key `{key_name}` (may be supported in a future version)"
    );
    if gctx.nightly_features_allowed {
        let _ = write!(
            message,
            "

consider passing `-Z{feature_name}` to enable this feature."
        );
    } else {
        let _ = write!(
            message,
            "

this Cargo does not support nightly features, but if you
switch to nightly channel you can pass
`-Z{feature_name}` to enable this feature.",
        );
    }
    warnings.push(message);
}

fn lints_to_rustflags(lints: &manifest::TomlLints) -> Vec<String> {
    let mut rustflags = lints
        .iter()
        // We don't want to pass any of the `cargo` lints to `rustc`
        .filter(|(tool, _)| tool != &"cargo")
        .flat_map(|(tool, lints)| {
            lints.iter().map(move |(name, config)| {
                let flag = match config.level() {
                    manifest::TomlLintLevel::Forbid => "--forbid",
                    manifest::TomlLintLevel::Deny => "--deny",
                    manifest::TomlLintLevel::Warn => "--warn",
                    manifest::TomlLintLevel::Allow => "--allow",
                };

                let option = if tool == "rust" {
                    format!("{flag}={name}")
                } else {
                    format!("{flag}={tool}::{name}")
                };
                (
                    config.priority(),
                    // Since the most common group will be `all`, put it last so people are more
                    // likely to notice that they need to use `priority`.
                    std::cmp::Reverse(name),
                    option,
                )
            })
        })
        .collect::<Vec<_>>();
    rustflags.sort();
    rustflags.into_iter().map(|(_, _, option)| option).collect()
}

fn emit_diagnostic(
    e: toml_edit::de::Error,
    contents: &str,
    manifest_file: &Path,
    gctx: &GlobalContext,
) -> anyhow::Error {
    let Some(span) = e.span() else {
        return e.into();
    };

    // Get the path to the manifest, relative to the cwd
    let manifest_path = diff_paths(manifest_file, gctx.cwd())
        .unwrap_or_else(|| manifest_file.to_path_buf())
        .display()
        .to_string();
    let message = Level::Error.title(e.message()).snippet(
        Snippet::source(contents)
            .origin(&manifest_path)
            .fold(true)
            .annotation(Level::Error.span(span)),
    );
    let renderer = Renderer::styled().term_width(
        gctx.shell()
            .err_width()
            .diagnostic_terminal_width()
            .unwrap_or(annotate_snippets::renderer::DEFAULT_TERM_WIDTH),
    );
    if let Err(err) = writeln!(gctx.shell().err(), "{}", renderer.render(message)) {
        return err.into();
    }
    return AlreadyPrintedError::new(e.into()).into();
}

/// Warn about paths that have been deprecated and may conflict.
fn warn_on_deprecated(new_path: &str, name: &str, kind: &str, warnings: &mut Vec<String>) {
    let old_path = new_path.replace("-", "_");
    warnings.push(format!(
        "conflicting between `{new_path}` and `{old_path}` in the `{name}` {kind}.\n
        `{old_path}` is ignored and not recommended for use in the future"
    ))
}

fn warn_on_unused(unused: &BTreeSet<String>, warnings: &mut Vec<String>) {
    for key in unused {
        warnings.push(format!("unused manifest key: {}", key));
        if key == "profiles.debug" {
            warnings.push("use `[profile.dev]` to configure debug builds".to_string());
        }
    }
}

fn unused_dep_keys(
    dep_name: &str,
    kind: &str,
    unused_keys: Vec<String>,
    warnings: &mut Vec<String>,
) {
    for unused in unused_keys {
        let key = format!("unused manifest key: {kind}.{dep_name}.{unused}");
        warnings.push(key);
    }
}

pub fn prepare_for_publish(me: &Package, ws: &Workspace<'_>) -> CargoResult<Package> {
    let contents = me.manifest().contents();
    let document = me.manifest().document();
    let original_toml = prepare_toml_for_publish(me.manifest().resolved_toml(), ws, me.root())?;
    let resolved_toml = original_toml.clone();
    let features = me.manifest().unstable_features().clone();
    let workspace_config = me.manifest().workspace_config().clone();
    let source_id = me.package_id().source_id();
    let mut warnings = Default::default();
    let mut errors = Default::default();
    let gctx = ws.gctx();
    let manifest = to_real_manifest(
        contents.to_owned(),
        document.clone(),
        original_toml,
        resolved_toml,
        features,
        workspace_config,
        source_id,
        me.manifest_path(),
        gctx,
        &mut warnings,
        &mut errors,
    )?;
    let new_pkg = Package::new(manifest, me.manifest_path());
    Ok(new_pkg)
}

/// Prepares the manifest for publishing.
// - Path and git components of dependency specifications are removed.
// - License path is updated to point within the package.
fn prepare_toml_for_publish(
    me: &manifest::TomlManifest,
    ws: &Workspace<'_>,
    package_root: &Path,
) -> CargoResult<manifest::TomlManifest> {
    let gctx = ws.gctx();

    if me
        .cargo_features
        .iter()
        .flat_map(|f| f.iter())
        .any(|f| f == "open-namespaces")
    {
        anyhow::bail!("cannot publish with `open-namespaces`")
    }

    let mut package = me.package().unwrap().clone();
    package.workspace = None;
    if let Some(StringOrBool::String(path)) = &package.build {
        let path = paths::normalize_path(Path::new(path));
        let path = path
            .into_os_string()
            .into_string()
            .map_err(|_err| anyhow::format_err!("non-UTF8 `package.build`"))?;
        package.build = Some(StringOrBool::String(normalize_path_string_sep(path)));
    }
    let current_resolver = package
        .resolver
        .as_ref()
        .map(|r| ResolveBehavior::from_manifest(r))
        .unwrap_or_else(|| {
            package
                .edition
                .as_ref()
                .and_then(|e| e.as_value())
                .map(|e| Edition::from_str(e))
                .unwrap_or(Ok(Edition::Edition2015))
                .map(|e| e.default_resolve_behavior())
        })?;
    if ws.resolve_behavior() != current_resolver {
        // This ensures the published crate if built as a root (e.g. `cargo install`) will
        // use the same resolver behavior it was tested with in the workspace.
        // To avoid forcing a higher MSRV we don't explicitly set this if it would implicitly
        // result in the same thing.
        package.resolver = Some(ws.resolve_behavior().to_manifest());
    }
    if let Some(license_file) = &package.license_file {
        let license_file = license_file
            .as_value()
            .context("license file should have been resolved before `prepare_for_publish()`")?;
        let license_path = Path::new(&license_file);
        let abs_license_path = paths::normalize_path(&package_root.join(license_path));
        if let Ok(license_file) = abs_license_path.strip_prefix(package_root) {
            package.license_file = Some(manifest::InheritableField::Value(
                normalize_path_string_sep(
                    license_file
                        .to_str()
                        .ok_or_else(|| anyhow::format_err!("non-UTF8 `package.license-file`"))?
                        .to_owned(),
                ),
            ));
        } else {
            // This path points outside of the package root. `cargo package`
            // will copy it into the root, so adjust the path to this location.
            package.license_file = Some(manifest::InheritableField::Value(
                license_path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
            ));
        }
    }

    if let Some(readme) = &package.readme {
        let readme = readme
            .as_value()
            .context("readme should have been resolved before `prepare_for_publish()`")?;
        match readme {
            manifest::StringOrBool::String(readme) => {
                let readme_path = Path::new(&readme);
                let abs_readme_path = paths::normalize_path(&package_root.join(readme_path));
                if let Ok(readme_path) = abs_readme_path.strip_prefix(package_root) {
                    package.readme = Some(manifest::InheritableField::Value(StringOrBool::String(
                        normalize_path_string_sep(
                            readme_path
                                .to_str()
                                .ok_or_else(|| {
                                    anyhow::format_err!("non-UTF8 `package.license-file`")
                                })?
                                .to_owned(),
                        ),
                    )));
                } else {
                    // This path points outside of the package root. `cargo package`
                    // will copy it into the root, so adjust the path to this location.
                    package.readme = Some(manifest::InheritableField::Value(
                        manifest::StringOrBool::String(
                            readme_path
                                .file_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_string(),
                        ),
                    ));
                }
            }
            manifest::StringOrBool::Bool(_) => {}
        }
    }

    let lib = if let Some(target) = &me.lib {
        Some(prepare_target_for_publish(target, "library")?)
    } else {
        None
    };
    let bin = prepare_targets_for_publish(me.bin.as_ref(), "binary")?;
    let example = prepare_targets_for_publish(me.example.as_ref(), "example")?;
    let test = prepare_targets_for_publish(me.test.as_ref(), "test")?;
    let bench = prepare_targets_for_publish(me.bench.as_ref(), "benchmark")?;

    let all = |_d: &manifest::TomlDependency| true;
    let mut manifest = manifest::TomlManifest {
        package: Some(package),
        project: None,
        profile: me.profile.clone(),
        lib,
        bin,
        example,
        test,
        bench,
        dependencies: map_deps(gctx, me.dependencies.as_ref(), all)?,
        dev_dependencies: map_deps(
            gctx,
            me.dev_dependencies(),
            manifest::TomlDependency::is_version_specified,
        )?,
        dev_dependencies2: None,
        build_dependencies: map_deps(gctx, me.build_dependencies(), all)?,
        build_dependencies2: None,
        features: me.features.clone(),
        target: match me.target.as_ref().map(|target_map| {
            target_map
                .iter()
                .map(|(k, v)| {
                    Ok((
                        k.clone(),
                        manifest::TomlPlatform {
                            dependencies: map_deps(gctx, v.dependencies.as_ref(), all)?,
                            dev_dependencies: map_deps(
                                gctx,
                                v.dev_dependencies(),
                                manifest::TomlDependency::is_version_specified,
                            )?,
                            dev_dependencies2: None,
                            build_dependencies: map_deps(gctx, v.build_dependencies(), all)?,
                            build_dependencies2: None,
                        },
                    ))
                })
                .collect()
        }) {
            Some(Ok(v)) => Some(v),
            Some(Err(e)) => return Err(e),
            None => None,
        },
        replace: None,
        patch: None,
        workspace: None,
        badges: me.badges.clone(),
        cargo_features: me.cargo_features.clone(),
        lints: me.lints.clone(),
        _unused_keys: Default::default(),
    };
    strip_features(&mut manifest);
    return Ok(manifest);

    fn strip_features(manifest: &mut TomlManifest) {
        fn insert_dep_name(
            dep_name_set: &mut BTreeSet<manifest::PackageName>,
            deps: Option<&BTreeMap<manifest::PackageName, manifest::InheritableDependency>>,
        ) {
            let Some(deps) = deps else {
                return;
            };
            deps.iter().for_each(|(k, _v)| {
                dep_name_set.insert(k.clone());
            });
        }
        let mut dep_name_set = BTreeSet::new();
        insert_dep_name(&mut dep_name_set, manifest.dependencies.as_ref());
        insert_dep_name(&mut dep_name_set, manifest.dev_dependencies());
        insert_dep_name(&mut dep_name_set, manifest.build_dependencies());
        if let Some(target_map) = manifest.target.as_ref() {
            target_map.iter().for_each(|(_k, v)| {
                insert_dep_name(&mut dep_name_set, v.dependencies.as_ref());
                insert_dep_name(&mut dep_name_set, v.dev_dependencies());
                insert_dep_name(&mut dep_name_set, v.build_dependencies());
            });
        }
        let features = manifest.features.as_mut();

        let Some(features) = features else {
            return;
        };

        features.values_mut().for_each(|feature_deps| {
            feature_deps.retain(|feature_dep| {
                let feature_value = FeatureValue::new(InternedString::new(feature_dep));
                match feature_value {
                    FeatureValue::Dep { dep_name } | FeatureValue::DepFeature { dep_name, .. } => {
                        let k = &manifest::PackageName::new(dep_name.to_string()).unwrap();
                        dep_name_set.contains(k)
                    }
                    _ => true,
                }
            });
        });
    }

    fn map_deps(
        gctx: &GlobalContext,
        deps: Option<&BTreeMap<manifest::PackageName, manifest::InheritableDependency>>,
        filter: impl Fn(&manifest::TomlDependency) -> bool,
    ) -> CargoResult<Option<BTreeMap<manifest::PackageName, manifest::InheritableDependency>>> {
        let Some(deps) = deps else {
            return Ok(None);
        };
        let deps = deps
            .iter()
            .filter(|(_k, v)| {
                if let manifest::InheritableDependency::Value(def) = v {
                    filter(def)
                } else {
                    false
                }
            })
            .map(|(k, v)| Ok((k.clone(), map_dependency(gctx, v)?)))
            .collect::<CargoResult<BTreeMap<_, _>>>()?;
        Ok(Some(deps))
    }

    fn map_dependency(
        gctx: &GlobalContext,
        dep: &manifest::InheritableDependency,
    ) -> CargoResult<manifest::InheritableDependency> {
        let dep = match dep {
            manifest::InheritableDependency::Value(manifest::TomlDependency::Detailed(d)) => {
                let mut d = d.clone();
                // Path dependencies become crates.io deps.
                d.path.take();
                // Same with git dependencies.
                d.git.take();
                d.branch.take();
                d.tag.take();
                d.rev.take();
                // registry specifications are elaborated to the index URL
                if let Some(registry) = d.registry.take() {
                    d.registry_index = Some(gctx.get_registry_index(&registry)?.to_string());
                }
                Ok(d)
            }
            manifest::InheritableDependency::Value(manifest::TomlDependency::Simple(s)) => {
                Ok(manifest::TomlDetailedDependency {
                    version: Some(s.clone()),
                    ..Default::default()
                })
            }
            _ => unreachable!(),
        };
        dep.map(manifest::TomlDependency::Detailed)
            .map(manifest::InheritableDependency::Value)
    }
}

fn prepare_targets_for_publish(
    targets: Option<&Vec<manifest::TomlTarget>>,
    context: &str,
) -> CargoResult<Option<Vec<manifest::TomlTarget>>> {
    let Some(targets) = targets else {
        return Ok(None);
    };

    let mut prepared = Vec::with_capacity(targets.len());
    for target in targets {
        let target = prepare_target_for_publish(target, context)?;
        prepared.push(target);
    }

    Ok(Some(prepared))
}

fn prepare_target_for_publish(
    target: &manifest::TomlTarget,
    context: &str,
) -> CargoResult<manifest::TomlTarget> {
    let mut target = target.clone();
    if let Some(path) = target.path {
        let path = normalize_path(&path.0);
        target.path = Some(manifest::PathValue(normalize_path_sep(path, context)?));
    }
    Ok(target)
}

fn normalize_path_sep(path: PathBuf, context: &str) -> CargoResult<PathBuf> {
    let path = path
        .into_os_string()
        .into_string()
        .map_err(|_err| anyhow::format_err!("non-UTF8 path for {context}"))?;
    let path = normalize_path_string_sep(path);
    Ok(path.into())
}

fn normalize_path_string_sep(path: String) -> String {
    if std::path::MAIN_SEPARATOR != '/' {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    } else {
        path
    }
}
