//! A source that wraps another source and applies patches.
//! See [`PatchedSource`] for details.

use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use cargo_util::paths;
use cargo_util_schemas::core::PatchChecksum;

use crate::CargoResult;
use crate::GlobalContext;
use crate::workspace::Dependency;
use crate::workspace::Package;
use crate::workspace::PackageId;
use crate::workspace::PackageSet;
use crate::workspace::SourceId;
use crate::workspace::SourceKind;
use crate::workspace::package::Downloads;
use crate::sources::IndexSummary;
use crate::sources::PathSource;
use crate::sources::SourceConfigMap;
use crate::sources::source::MaybePackage;
use crate::sources::source::QueryKind;
use crate::sources::source::Source;
use crate::sources::source::SourceMap;

use crate::util::cache_lock::CacheLockMode;
use crate::util::data_structures::HashMap;
use crate::util::hex;
use crate::util::patch::apply_patch_file;

/// A file indicates that if present, the patched source is ready to use.
///
/// This protects against interrupted operations.
const READY_LOCK: &str = ".cargo-ok";

/// `PatchedSource` is a source that, when querying index,
/// it patches a paticular package with given local patch files.
///
/// This could only be created from [the `[patch]` section][patch]
/// with any entry carrying `{ .., patches = ["..."] }` field.
///
/// [patch]: https://doc.rust-lang.org/nightly/cargo/reference/overriding-dependencies.html#the-patch-section
///
/// ## Filesystem layout
///
/// When Cargo fetches a package from a `PatchedSource`,
/// it'll copy everything from the original source to a dedicated patched source directory.
///
/// * For registry sources: `<patched-src>/<ident>-<hash>/<name>-<version>/<patch-cksum>/`
/// * For git sources: `<patched-src>/<ident>-<hash>/<short-rev>/<patch-cksum>/`
///
/// ```text
/// $CARGO_HOME/patched-src/github.com-6d038ece37e82ae2
/// ├── gimli-0.29.0/
/// │  ├── a0d193bd15a5ed96/    # checksum of all patch files to gimli@0.29.0
/// │  ├── c58e1db3de7c154d/
/// └── serde-1.0.197/
///    └── deadbeef12345678/
/// ```
///
/// ## `SourceId` for tracking the original package
///
/// Due to the nature that a patched source is actually locked to a specific version of one package,
/// the SourceId URL of a `PatchedSource` needs to carry such information.
/// It looks like:
///
/// ```text
/// patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c#bar@1.0.0
/// ```
///
/// The URL format of patched source is the underlying SourceID URL with `patched+` prefix,
/// plus extra URL querying string of `patch-cksum=<checksum-of-all-patch-files>`.
pub struct PatchedSource<'gctx> {
    /// The patched source ID.
    source_id: SourceId,
    /// The source being patched.
    to_patch: Rc<dyn Source + 'gctx>,
    /// Patch file paths.
    patch_paths: Vec<PathBuf>,
    /// Cached patched packages, keyed by the original package IDs before patching.
    patched_packages: RefCell<HashMap<PackageId, Package>>,
    gctx: &'gctx GlobalContext,
}

impl<'gctx> PatchedSource<'gctx> {
    pub fn new(
        id_to_patch: SourceId,
        gctx: &'gctx GlobalContext,
    ) -> CargoResult<PatchedSource<'gctx>> {
        let SourceKind::Patched(cksum) = id_to_patch.kind() else {
            unreachable!("must be patched source kind");
        };

        let patch_paths = gctx
            .get_patch_paths(cksum)
            .expect("patch paths must ber registered");

        let map = SourceConfigMap::new(gctx)?;
        let to_patch = SourceId::from_url(id_to_patch.url().as_str())?;
        let to_patch: Rc<dyn Source + 'gctx> = map.load(to_patch)?.into();

        Ok(PatchedSource {
            source_id: id_to_patch,
            to_patch,
            patch_paths,
            patched_packages: RefCell::new(HashMap::default()),
            gctx,
        })
    }

    /// Takes the original package,
    /// copies its source,
    /// applies patches,
    /// and returns the patched package.
    fn patch_pkg(&self, orig_pkg: &Package) -> CargoResult<Package> {
        let orig_pkg_id = orig_pkg.package_id();

        let is_git = self.to_patch.source_id().is_git();
        let (src_root, pkg_subdir) = if is_git {
            // For git sources,
            // we need to copy the entire repo to preserve workspace inheritance.
            let repo_root = find_git_checkout_root(self.gctx, orig_pkg.root());
            let subdir = orig_pkg
                .root()
                .strip_prefix(&repo_root)
                .expect("package root is under git checkout")
                .to_path_buf();
            (repo_root, Some(subdir))
        } else {
            // For registry sources,
            // we copy only the package directory.
            (orig_pkg.root().to_path_buf(), None)
        };

        let dst = self.patched_src_dir(orig_pkg_id, is_git)?;
        let ready_lock = dst.join(READY_LOCK);

        if !ready_lock.exists() {
            if dst.exists() {
                paths::remove_dir_all(&dst)?;
            }
            copy_pkg_src(&src_root, &dst)?;
            self.apply_patches(orig_pkg_id, &dst)?;

            paths::write(&ready_lock, "")?;
        }

        // For git sources, the package is in a subdirectory of the patched repo
        let pkg_root = match pkg_subdir {
            Some(ref sub) if !sub.as_os_str().is_empty() => dst.join(sub),
            _ => dst,
        };
        PathSource::new(&pkg_root, self.source_id, self.gctx).root_package()
    }

    async fn download_pkg(&self, pkg_id_to_patch: PackageId) -> CargoResult<Package> {
        let mut sources = SourceMap::new();
        sources.insert_shared(Rc::clone(&self.to_patch));
        let pkg_set = PackageSet::new(&[pkg_id_to_patch], sources, self.gctx)?;
        let pkgs = Downloads::download(&pkg_set, [pkg_id_to_patch]).await?;
        Ok(Package::clone(pkgs[0]))
    }


    fn apply_patches(&self, pkg_id: PackageId, dst: &Path) -> CargoResult<()> {
        let patch_files = self.patch_files();
        assert!(!patch_files.is_empty(), "must have at least one patch");

        let mut shell = self.gctx.shell();
        shell.status("Patching", pkg_id)?;

        for patch_file in patch_files {
            tracing::debug!(?patch_file, "apply patch to {pkg_id}");
            debug_assert!(patch_file.is_absolute());
            apply_patch_file(patch_file, dst)?;
        }

        Ok(())
    }

    /// Gets the destination directory we put the patched source at.
    ///
    /// * For registry sources: `<patched-src>/<ident>-<hash>/<name>-<version>/<patch-cksum>/`
    /// * For git sources: `<patched-src>/<ident>-<hash>/<short-rev>/<patch-cksum>/`
    fn patched_src_dir(&self, orig_pkg_id: PackageId, is_git: bool) -> CargoResult<PathBuf> {
        let patched_src_root = self.gctx.patched_source_path();
        let patched_src_root = self
            .gctx
            .assert_package_cache_locked(CacheLockMode::DownloadExclusive, &patched_src_root);
        let source_id = self.to_patch.source_id();
        let ident = source_id.url().host_str().unwrap_or_default();
        let hash = hex::short_hash(&source_id);
        let cksum = self.patch_checksum().as_str();
        let cksum = &cksum[..cksum.len().min(8)];

        let mut dst = patched_src_root.join(format!("{ident}-{hash}"));
        if is_git {
            let rev = orig_pkg_id
                .source_id()
                .precise_git_fragment()
                .expect("git package must have precise rev after fetched");
            dst.push(rev);
        } else {
            let name = orig_pkg_id.name();
            let version = orig_pkg_id.version();
            dst.push(format!("{name}-{version}"));
        };
        dst.push(cksum);
        Ok(dst)
    }

    fn patch_checksum(&self) -> &PatchChecksum {
        let SourceKind::Patched(cksum) = self.source_id.kind() else {
            unreachable!("must be patched source kind");
        };
        cksum
    }

    fn patch_files(&self) -> &[PathBuf] {
        &self.patch_paths
    }
}

#[async_trait::async_trait(?Send)]
impl<'gctx> Source for PatchedSource<'gctx> {
    fn source_id(&self) -> SourceId {
        self.source_id
    }

    fn supports_checksums(&self) -> bool {
        false
    }

    fn requires_precise(&self) -> bool {
        false
    }

    async fn query(
        &self,
        dep: &Dependency,
        kind: QueryKind,
        f: &mut dyn FnMut(IndexSummary),
    ) -> CargoResult<()> {
        let source_id_to_patch = self.to_patch.source_id();
        let dep_to_patch = dep.clone().map_source(self.source_id, source_id_to_patch);

        let summaries = self.to_patch.query_vec(&dep_to_patch, kind).await?;

        if summaries.len() != 1 {
            anyhow::bail!(
                "patch for `{}` matched {} candidates, but patches must match exactly one candidate",
                dep.package_name(),
                summaries.len()
            );
        }

        for summary in summaries {
            let pkg_id = summary.package_id();
            let orig_pkg = self.download_pkg(pkg_id).await?;
            let patched_pkg = self.patch_pkg(&orig_pkg)?;
            let patched_pkg_id = patched_pkg.package_id();

            if pkg_id.name() != patched_pkg_id.name()
                || pkg_id.version() != patched_pkg_id.version()
            {
                anyhow::bail!(
                    "patch for `{}` must not change the package name or version\n\
                    note: original package is `{}`, but after patching it became `{}`",
                    dep.package_name(),
                    pkg_id,
                    patched_pkg_id
                );
            }

            // Cache the patched package for download()
            let patched_summary = patched_pkg.summary().clone();
            self.patched_packages
                .borrow_mut()
                .insert(pkg_id, patched_pkg);

            f(IndexSummary::Candidate(patched_summary));
        }

        Ok(())
    }

    fn invalidate_cache(&self) {
        self.to_patch.invalidate_cache();
    }

    fn set_quiet(&mut self, quiet: bool) {
        if let Some(to_patch) = Rc::get_mut(&mut self.to_patch) {
            to_patch.set_quiet(quiet);
        }
    }

    async fn download(&self, id: PackageId) -> CargoResult<MaybePackage> {
        let to_patch = self.to_patch.source_id();
        let patch_with = id.with_source_id(to_patch);
        let msg = "patched source downloads package during query";
        let packages = self.patched_packages.borrow();
        let pkg = packages.get(&patch_with).expect(msg);
        assert_eq!(pkg.package_id(), id, "{msg}");

        Ok(MaybePackage::Ready(pkg.clone()))
    }

    async fn finish_download(
        &self,
        _pkg_id: PackageId,
        _contents: Vec<u8>,
    ) -> CargoResult<Package> {
        panic!("patched source downloads package during query")
    }

    fn fingerprint(&self, pkg: &Package) -> CargoResult<String> {
        let fingerprint = self.to_patch.fingerprint(&pkg)?;
        let cksum = self.patch_checksum().as_str();
        Ok(format!("{fingerprint}/{cksum}"))
    }

    fn describe(&self) -> String {
        let n = self.patch_files().len();
        let plural = if n == 1 { "" } else { "s" };
        let desc = self.to_patch.describe();
        format!("{desc} with {n} patch file{plural}",)
    }
}

/// Copies the source tree at `src` into `dst`, ready to be patched.
fn copy_pkg_src(src: &Path, dst: &Path) -> CargoResult<()> {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        let path = entry.path().strip_prefix(src).unwrap();

        // The original source carries its own `.cargo-ok` at the root
        // (`PACKAGE_SOURCE_LOCK` / `CHECKOUT_READY_LOCK`). Copying it would
        // pre-create our `READY_LOCK`, making an interrupted or failed patch
        // look already-applied and silently yielding an unpatched build.
        if path == Path::new(READY_LOCK) {
            continue;
        }

        let dst_path = dst.join(path);

        if entry.file_type().is_dir() {
            paths::create_dir_all(&dst_path)?;
        } else {
            if entry.file_type().is_symlink() {
                ensure_symlink_target_inside(src, entry.path())?;
            }
            paths::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

fn ensure_symlink_target_inside(src_root: &Path, symlink: &Path) -> CargoResult<()> {
    // Both paths must be canonicalized for a reliable prefix comparison.
    let src_root = crate::util::try_canonicalize(src_root)?;
    let target = crate::util::try_canonicalize(symlink)?;
    if !target.starts_with(&src_root) {
        anyhow::bail!(
            "patched source symlink `{}` points outside source root\n\
            note: symlink target resolves to `{}`\n\
            help: replace the symlink with a copy of the target file",
            symlink.display(),
            target.display()
        );
    }

    Ok(())
}

/// Finds the git checkout root for a package path.
fn find_git_checkout_root(gctx: &GlobalContext, pkg_root: &Path) -> PathBuf {
    let checkouts_path = gctx.git_checkouts_path();
    let checkouts_path = checkouts_path.as_path_unlocked();

    // Invariant: Git checkouts have a fixed structure: `checkouts/<ident>/<short-rev>/`.
    // The package root is always under the checkouts path.
    let relative = pkg_root
        .strip_prefix(checkouts_path)
        .expect("git package root is under checkouts path");

    let mut components = relative.components();
    let msg = "invalid git checkout path structure";
    let ident = components.next().expect(msg);
    let short_rev = components.next().expect(msg);

    checkouts_path.join(ident).join(short_rev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_pkg_src_skips_source_ready_lock() {
        // The original source's `.cargo-ok` must not be copied, otherwise it
        // would masquerade as our `READY_LOCK` and make an unpatched copy look
        // already-patched.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        paths::create_dir_all(src.join("nested")).unwrap();
        paths::write(src.join(READY_LOCK), "from source").unwrap();
        paths::write(src.join("Cargo.toml"), "[package]").unwrap();
        paths::write(src.join("nested/lib.rs"), "code").unwrap();

        copy_pkg_src(&src, &dst).unwrap();

        assert!(
            !dst.join(READY_LOCK).exists(),
            "source `.cargo-ok` must not be copied into the patched dir"
        );
        assert!(dst.join("Cargo.toml").exists());
        assert!(dst.join("nested/lib.rs").exists());
    }
}
