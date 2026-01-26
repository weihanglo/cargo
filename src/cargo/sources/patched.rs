//! A source that wraps another source and applies patches.
//! See [`PatchedSource`] for details.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::task::Poll;

use cargo_util::paths;
use cargo_util_schemas::core::PatchChecksum;

use crate::CargoResult;
use crate::GlobalContext;
use crate::core::Dependency;
use crate::core::Package;
use crate::core::PackageId;
use crate::core::PackageSet;
use crate::core::SourceId;
use crate::core::SourceKind;
use crate::sources::IndexSummary;
use crate::sources::PathSource;
use crate::sources::SourceConfigMap;
use crate::sources::source::MaybePackage;
use crate::sources::source::QueryKind;
use crate::sources::source::Source;
use crate::sources::source::SourceMap;

use crate::util::cache_lock::CacheLockMode;
use crate::util::hex;
use crate::util::patch::apply_patch_file;

/// A file indicates that if present, the patched source is ready to use.
const READY_LOCK: &str = ".cargo-ok";

/// `PatchedSource` is a source that, when querying index, it patches a paticular
/// package with given local patch files.
///
/// This could only be created from [the `[patch]` section][patch] with any
/// entry carrying `{ .., patches = ["..."] }` field. Other kinds of dependency
/// sections (normal, dev, build) shouldn't allow to create any `PatchedSource`.
///
/// [patch]: https://doc.rust-lang.org/nightly/cargo/reference/overriding-dependencies.html#the-patch-section
///
/// ## Filesystem layout
///
/// When Cargo fetches a package from a `PatchedSource`, it'll copy everything
/// from the original source to a dedicated patched source directory. That
/// directory is located under `$CARGO_HOME`. The patched source of each package
/// would be put under:
///
/// ```text
/// $CARGO_HOME/patched-src/<hash-of-original-source>/<pkg>-<version>/<cksum-of-patches>/`.
/// ```
///
/// The file tree of the patched source directory roughly looks like:
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
/// Due to the nature that a patched source is actually locked to a specific
/// version of one package, the SourceId URL of a `PatchedSource` needs to
/// carry such information. It looks like:
///
/// ```text
/// patched+registry+https://github.com/rust-lang/crates.io-index?patch=0001-bugfix.patch&patch=0002-feat.patch
/// ```
///
/// where the `patched+` protocol is essential for Cargo to distinguish between
/// a patched source and the source it patches, with extra URL query string of
/// `patch-cksum=<checksum>`.
///
/// Patch file paths are stored in [`PatchedInfo`] at runtime, not in the URL.
pub struct PatchedSource<'gctx> {
    /// The patched source ID (patched+registry+...).
    source_id: SourceId,
    /// The source being patched.
    to_patch: Option<Box<dyn Source + 'gctx>>,
    /// Patch file paths.
    patch_paths: Vec<PathBuf>,
    /// Cached patched packages, keyed by package IDs being patched.
    patched_packages: HashMap<PackageId, Package>,
    gctx: &'gctx GlobalContext,
}

impl<'gctx> PatchedSource<'gctx> {
    pub fn new(
        patched_source_id: SourceId,
        gctx: &'gctx GlobalContext,
    ) -> CargoResult<PatchedSource<'gctx>> {
        let SourceKind::Patched(cksum) = patched_source_id.kind() else {
            unreachable!("must be patched source kind");
        };

        // Get patch file paths from GlobalContext (store to avoid repeated mutex lookups)
        let patch_paths = gctx.get_patch_paths(cksum).ok_or_else(|| {
            let cksum = cksum.as_str();
            let cksum = &cksum[..cksum.len().min(8)];
            anyhow::anyhow!(
                "no patch files registered for checksum `{cksum}`\n\n
                note: patch files must be registered via [patch] table",
            )
        })?;

        let map = SourceConfigMap::new(gctx)?;
        let to_patch = SourceId::from_url(patched_source_id.url().as_str())?;
        let to_patch = Some(map.load(to_patch, &Default::default())?);

        Ok(PatchedSource {
            source_id: patched_source_id,
            to_patch,
            patch_paths,
            patched_packages: HashMap::new(),
            gctx,
        })
    }

    fn download_and_patch(&mut self, orig_pkg_id: PackageId) -> CargoResult<Package> {
        let patched_pkg_id = orig_pkg_id.with_source_id(self.source_id);
        let orig_pkg = self.download_pkg(orig_pkg_id)?;

        // For git sources, we need to copy the entire repo to preserve workspace inheritance.
        // For registry sources, we copy only the package directory.
        let is_git = self.source_to_patch().source_id().is_git();
        let (src_root, pkg_subdir) = if is_git {
            let repo_root = find_git_repo_root(orig_pkg.root())?;
            let subdir = orig_pkg
                .root()
                .strip_prefix(&repo_root)
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            (repo_root, Some(subdir))
        } else {
            (orig_pkg.root().to_path_buf(), None)
        };

        let dst = self.patched_src_dir(patched_pkg_id, is_git)?;
        let ready_lock = dst.join(READY_LOCK);

        // ready_lock is a completion marker - if it exists, patching was successful.
        // The checksum is already part of the directory path, so we only need
        // to check for existence (protects against interrupted operations).
        if !ready_lock.exists() {
            if dst.exists() {
                paths::remove_dir_all(&dst)?;
            }
            self.copy_pkg_src(&src_root, &dst)?;
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

    fn download_pkg(&mut self, pkg_id_to_patch: PackageId) -> CargoResult<Package> {
        // Download operation consumes source,
        // so we need to get it from Option and put it back.
        let source_id = self.source_to_patch().source_id();
        let source = self.to_patch.take().expect("must have source");

        let mut sources = SourceMap::new();
        sources.insert(source);
        let pkg_set = PackageSet::new(&[pkg_id_to_patch], sources, self.gctx)?;
        let pkg = pkg_set.get_one(pkg_id_to_patch)?;

        let source = pkg_set.sources_mut().remove(source_id).unwrap();
        assert!(self.to_patch.replace(source).is_none());

        Ok(Package::clone(pkg))
    }

    /// Copies code to the destination we put the patched source at.
    fn copy_pkg_src(&self, src: &Path, dst: &Path) -> CargoResult<()> {
        for entry in walkdir::WalkDir::new(src) {
            let entry = entry?;
            let path = entry.path().strip_prefix(src).unwrap();
            let dst_path = dst.join(path);

            if entry.file_type().is_dir() {
                paths::create_dir_all(&dst_path)?;
            } else {
                // TODO: handle symlink?
                paths::copy(entry.path(), &dst_path)?;
            }
        }
        Ok(())
    }

    fn apply_patches(&self, pkg_id: PackageId, dst: &Path) -> CargoResult<()> {
        let patch_files = self.patch_files();
        assert!(!patch_files.is_empty(), "must have at least one patch");

        let mut shell = self.gctx.shell();
        shell.status("Patching", pkg_id)?;

        for patch_file in patch_files {
            // TODO: patch paths are relative to workspace root, but we use cwd here.
            // This breaks when running cargo from a subdirectory.
            // See design doc for options to fix this.
            let patch_file = self.gctx.cwd().join(patch_file);
            apply_patch_file(&patch_file, dst)?;
        }

        Ok(())
    }

    /// Gets the destination directory we put the patched source at.
    ///
    /// For registry sources: `<patched-src>/<url-hash>/<name>-<version>/<patch-cksum>/`
    /// For git sources: `<patched-src>/<url-hash>-<rev>-<patch-cksum>/` (per-repo, not per-package)
    fn patched_src_dir(&self, pkg_id: PackageId, is_git: bool) -> CargoResult<PathBuf> {
        let patched_src_root = self.gctx.patched_source_path();
        let patched_src_root = self
            .gctx
            .assert_package_cache_locked(CacheLockMode::DownloadExclusive, &patched_src_root);
        let source_id = self.source_to_patch().source_id();
        let ident = source_id.url().host_str().unwrap_or_default();
        let hash = hex::short_hash(&source_id);
        let cksum = self.patch_checksum().as_str();
        let cksum = &cksum[..cksum.len().min(8)];

        let dst = if is_git {
            // For git: per-repo directory keyed by url + rev + patches
            let rev = source_id.precise_git_fragment().unwrap_or("HEAD");
            patched_src_root.join(format!("{ident}-{hash}-{rev}-{cksum}"))
        } else {
            // For registry: per-package directory
            let name = pkg_id.name();
            let version = pkg_id.version();
            let mut dst = patched_src_root.join(format!("{ident}-{hash}"));
            dst.push(format!("{name}-{version}"));
            dst.push(cksum);
            dst
        };
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

    fn source_to_patch(&self) -> &dyn Source {
        self.to_patch.as_ref().expect("must have source")
    }

    fn source_to_patch_mut(&mut self) -> &mut dyn Source {
        self.to_patch.as_mut().expect("must have source")
    }
}

/// Finds the git repository root by walking up from the given path.
/// Returns the path containing the `.git` directory.
fn find_git_repo_root(start: &Path) -> CargoResult<PathBuf> {
    let mut current = start;
    loop {
        if current.join(".git").exists() {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => anyhow::bail!(
                "could not find git repository root starting from `{}`",
                start.display()
            ),
        }
    }
}

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

    fn query(
        &mut self,
        dep: &Dependency,
        kind: QueryKind,
        f: &mut dyn FnMut(IndexSummary),
    ) -> Poll<CargoResult<()>> {
        let source_id_to_patch = self.source_to_patch().source_id();
        let dep_to_patch = dep.clone().map_source(self.source_id, source_id_to_patch);

        let summaries = match self.source_to_patch_mut().query_vec(&dep_to_patch, kind) {
            Poll::Ready(Ok(summaries)) => summaries,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };

        if summaries.len() != 1 {
            return Poll::Ready(Err(anyhow::format_err!("!!!!!!!!!!!!")));
        }

        for summary in summaries {
            let pkg_id = summary.package_id();
            let patched_pkg = match self.download_and_patch(pkg_id) {
                Ok(pkg) => pkg,
                Err(e) => return Poll::Ready(Err(e)),
            };

            // Cache the patched package for download()
            let patched_summary = patched_pkg.summary().clone();
            self.patched_packages.insert(pkg_id, patched_pkg);

            f(IndexSummary::Candidate(patched_summary));
        }

        Poll::Ready(Ok(()))
    }

    fn invalidate_cache(&mut self) {
        self.source_to_patch_mut().invalidate_cache();
    }

    fn set_quiet(&mut self, quiet: bool) {
        self.source_to_patch_mut().set_quiet(quiet);
    }

    fn download(&mut self, id: PackageId) -> CargoResult<MaybePackage> {
        let to_patch = self.source_to_patch().source_id();
        let patch_with = id.with_source_id(to_patch);
        let msg = "patched source downloads package during query";
        let pkg = self.patched_packages.get(&patch_with).expect(msg);
        assert_eq!(pkg.package_id(), id, "{msg}");

        Ok(MaybePackage::Ready(pkg.clone()))
    }

    fn finish_download(&mut self, _pkg_id: PackageId, _contents: Vec<u8>) -> CargoResult<Package> {
        panic!("patched source downloads package during query")
    }

    fn fingerprint(&self, pkg: &Package) -> CargoResult<String> {
        let fingerprint = self.source_to_patch().fingerprint(&pkg)?;
        // Use full hash for fingerprint to maximize collision resistance
        let cksum = self.patch_checksum().as_str();
        Ok(format!("{fingerprint}/{cksum}"))
    }

    fn describe(&self) -> String {
        let n = self.patch_files().len();
        let plural = if n == 1 { "" } else { "s" };
        let desc = self.source_to_patch().describe();
        format!("{desc} with {n} patch file{plural}",)
    }

    fn add_to_yanked_whitelist(&mut self, _pkgs: &[PackageId]) {
        // There is no yanked package for a patched source
    }

    fn is_yanked(&mut self, _pkg: PackageId) -> Poll<CargoResult<bool>> {
        // There is no yanked package for a patched source
        Poll::Ready(Ok(false))
    }

    fn block_until_ready(&mut self) -> CargoResult<()> {
        self.source_to_patch_mut().block_until_ready()
    }
}
