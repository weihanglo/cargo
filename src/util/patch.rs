//! Utilities for applying unified diff patches.

use std::path::Path;

use cargo_util::paths;
use diffy::binary::BinaryPatch;
use diffy::patch_set::FileOperation;
use diffy::patch_set::ParseOptions;
use diffy::patch_set::PatchKind;
use diffy::patch_set::PatchSet;

use crate::CargoResult;

/// Applies a Git format patch file to a directory.
pub fn apply_patch_file(patch_file: &Path, dst: &Path) -> CargoResult<()> {
    let patch_content = paths::read_bytes(patch_file)?;

    let mut patches = PatchSet::parse_bytes(&patch_content, ParseOptions::gitdiff()).peekable();
    if matches!(patches.peek(), None | Some(Err(_))) {
        patches = PatchSet::parse_bytes(&patch_content, ParseOptions::unidiff()).peekable();
    }
    if patches.peek().is_none() {
        anyhow::bail!("no valid patches found in `{}`", patch_file.display());
    }

    for file_patch in patches {
        let file_patch = file_patch?;
        let operation = {
            let op = file_patch.operation();
            // Rename/Copy paths come from git headers without a/b prefix.
            let strip = match op {
                FileOperation::Rename { .. } | FileOperation::Copy { .. } => 0,
                _ => 1,
            };
            op.strip_prefix(strip)
        };

        match operation {
            FileOperation::Create(path) => {
                let target_path = dst.join(patch_path(path.as_ref())?);
                let patched = match file_patch.patch() {
                    PatchKind::Text(patch) => diffy::apply_bytes(&[], patch)?,
                    PatchKind::Binary(BinaryPatch::Marker) => continue,
                    PatchKind::Binary(patch) => patch.apply(&[])?,
                };
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                paths::write(&target_path, &patched)?;
            }
            FileOperation::Delete(path) => {
                paths::remove_file(dst.join(patch_path(path.as_ref())?))?;
            }
            FileOperation::Modify { original, modified } => {
                let source_path = dst.join(patch_path(original.as_ref())?);
                let target_path = dst.join(patch_path(modified.as_ref())?);
                let patched = match file_patch.patch() {
                    PatchKind::Text(patch) => {
                        let original = paths::read_bytes(&source_path)?;
                        diffy::apply_bytes(&original, patch)?
                    }
                    PatchKind::Binary(BinaryPatch::Marker) => continue,
                    PatchKind::Binary(patch) => {
                        let original = paths::read_bytes(&source_path)?;
                        patch.apply(&original)?
                    }
                };
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                paths::write(&target_path, &patched)?;
                if source_path != target_path {
                    // If renamed, remove the original file
                    paths::remove_file(&source_path)?;
                }
            }
            FileOperation::Rename { from, to } => {
                let source_path = dst.join(patch_path(from.as_ref())?);
                let target_path = dst.join(patch_path(to.as_ref())?);
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                std::fs::rename(source_path, target_path)?;
            }
            FileOperation::Copy { from, to } => {
                let source_path = dst.join(patch_path(from.as_ref())?);
                let target_path = dst.join(patch_path(to.as_ref())?);
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                std::fs::copy(source_path, target_path)?;
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
fn patch_path(path: &[u8]) -> CargoResult<&Path> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    contained_path(Path::new(OsStr::from_bytes(path)))
}

#[cfg(not(unix))]
fn patch_path(path: &[u8]) -> CargoResult<&Path> {
    contained_path(Path::new(std::str::from_utf8(path)?))
}

/// Rejects patch paths that would escape the destination directory.
///
/// Patch headers are attacker-controlled when building an untrusted project,
/// and every operation joins the path onto `dst`. An absolute path makes
/// `Path::join` discard `dst` entirely, and a `..` component walks outside it,
/// so either could write or delete arbitrary files during a build.
fn contained_path(path: &Path) -> CargoResult<&Path> {
    use std::path::Component;

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("path `{}` in patch escapes the package root", path.display());
            }
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests_apply_patch_file {
    use std::path::PathBuf;

    use snapbox::assert_data_eq;
    use snapbox::str;

    use super::*;

    struct Fixture {
        _tmp: tempfile::TempDir,
        patch_file: PathBuf,
        dst: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let patch_file = tmp.path().join("test.patch");
            let dst = tmp.path().join("dst");
            std::fs::create_dir_all(&dst).unwrap();
            Self {
                _tmp: tmp,
                patch_file,
                dst,
            }
        }

        fn write_file(&self, name: &str, content: &str) {
            let path = self.dst.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn write_file_bytes(&self, name: &str, content: &[u8]) {
            let path = self.dst.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn read_file(&self, name: &str) -> String {
            std::fs::read_to_string(self.dst.join(name)).unwrap()
        }

        fn read_file_bytes(&self, name: &str) -> Vec<u8> {
            std::fs::read(self.dst.join(name)).unwrap()
        }

        fn file_exists(&self, name: &str) -> bool {
            self.dst.join(name).exists()
        }

        fn write_patch(&self, content: &str) {
            std::fs::write(&self.patch_file, content).unwrap();
        }

        fn write_patch_bytes(&self, content: &[u8]) {
            std::fs::write(&self.patch_file, content).unwrap();
        }

        fn apply(&self) -> crate::CargoResult<()> {
            apply_patch_file(&self.patch_file, &self.dst)
        }
    }

    #[test]
    fn modify_file() {
        let f = Fixture::new();

        f.write_file("file.rs", "old\n");
        f.write_patch(
            "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1 +1 @@
-old
+new
",
        );

        f.apply().unwrap();
        assert_data_eq!(
            f.read_file("file.rs"),
            str![[r#"
new

"#]]
        );
    }

    #[test]
    fn create_file() {
        let f = Fixture::new();

        f.write_patch(
            "\
diff --git a/file.rs b/file.rs
new file mode 100644
--- /dev/null
+++ b/file.rs
@@ -0,0 +1 @@
+new
",
        );

        f.apply().unwrap();
        assert_data_eq!(
            f.read_file("file.rs"),
            str![[r#"
new

"#]]
        );
    }

    #[test]
    fn delete_file() {
        let f = Fixture::new();

        f.write_file("file.rs", "old\n");
        f.write_patch(
            "\
diff --git a/file.rs b/file.rs
deleted file mode 100644
--- a/file.rs
+++ /dev/null
@@ -1 +0,0 @@
-old
",
        );

        f.apply().unwrap();
        assert!(!f.file_exists("file.rs"));
    }

    #[test]
    fn modify_with_different_paths() {
        let f = Fixture::new();

        // A modify operation with different source/target paths removes the original
        f.write_file("from.rs", "content\n");
        f.write_patch(
            "\
diff --git a/from.rs b/to.rs
--- a/from.rs
+++ b/to.rs
@@ -1 +1 @@
-content
+content
",
        );

        f.apply().unwrap();
        assert!(!f.file_exists("from.rs"));
        assert_data_eq!(
            f.read_file("to.rs"),
            str![[r#"
content

"#]]
        );
    }

    #[test]
    fn pure_rename() {
        let f = Fixture::new();

        f.write_file("old.rs", "content\n");
        f.write_patch(
            "\
diff --git a/old.rs b/new.rs
similarity index 100%
rename from old.rs
rename to new.rs
",
        );

        f.apply().unwrap();
        assert!(!f.file_exists("old.rs"));
        assert_data_eq!(
            f.read_file("new.rs"),
            str![[r#"
content

"#]]
        );
    }

    #[test]
    fn pure_copy() {
        let f = Fixture::new();

        f.write_file("original.rs", "content\n");
        f.write_patch(
            "\
diff --git a/original.rs b/copy.rs
similarity index 100%
copy from original.rs
copy to copy.rs
",
        );

        f.apply().unwrap();
        // Original still exists
        assert!(f.file_exists("original.rs"));
        assert_data_eq!(
            f.read_file("copy.rs"),
            str![[r#"
content

"#]]
        );
    }

    #[test]
    fn multi_file_patch() {
        let f = Fixture::new();

        f.write_file("a.rs", "old1\n");
        f.write_file("b.rs", "old2\n");
        f.write_patch(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old1
+new1
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-old2
+new2
",
        );

        f.apply().unwrap();
        assert_data_eq!(
            f.read_file("a.rs"),
            str![[r#"
new1

"#]]
        );
        assert_data_eq!(
            f.read_file("b.rs"),
            str![[r#"
new2

"#]]
        );
    }

    #[test]
    fn creates_parent_directories() {
        let f = Fixture::new();

        f.write_patch(
            "\
diff --git a/a/b/c/file.rs b/a/b/c/file.rs
new file mode 100644
--- /dev/null
+++ b/a/b/c/file.rs
@@ -0,0 +1 @@
+new
",
        );

        f.apply().unwrap();
        assert_data_eq!(
            f.read_file("a/b/c/file.rs"),
            str![[r#"
new

"#]]
        );
    }

    #[test]
    fn rename_into_new_directory() {
        let f = Fixture::new();

        f.write_file("old.rs", "content\n");
        f.write_patch(
            "\
diff --git a/old.rs b/sub/dir/new.rs
similarity index 100%
rename from old.rs
rename to sub/dir/new.rs
",
        );

        f.apply().unwrap();
        assert!(!f.file_exists("old.rs"));
        assert_data_eq!(
            f.read_file("sub/dir/new.rs"),
            str![[r#"
content

"#]]
        );
    }

    #[test]
    fn copy_into_new_directory() {
        let f = Fixture::new();

        f.write_file("original.rs", "content\n");
        f.write_patch(
            "\
diff --git a/original.rs b/sub/dir/copy.rs
similarity index 100%
copy from original.rs
copy to sub/dir/copy.rs
",
        );

        f.apply().unwrap();
        assert!(f.file_exists("original.rs"));
        assert_data_eq!(
            f.read_file("sub/dir/copy.rs"),
            str![[r#"
content

"#]]
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let f = Fixture::new();

        f.write_patch(
            "\
diff --git a/../escape.rs b/../escape.rs
new file mode 100644
--- /dev/null
+++ b/../escape.rs
@@ -0,0 +1 @@
+escaped
",
        );

        let err = f.apply().unwrap_err();
        assert!(err.to_string().contains("escapes the package root"), "{err}");
        // Nothing was written outside `dst`.
        assert!(!f.dst.parent().unwrap().join("escape.rs").exists());
    }

    #[test]
    fn empty_patch_is_error() {
        let f = Fixture::new();

        f.write_file("file.rs", "unchanged\n");
        f.write_patch("");

        let err = f.apply().unwrap_err();
        assert!(err.to_string().contains("no valid patches found"));
    }

    #[test]
    fn binary_marker_skipped() {
        let f = Fixture::new();

        f.write_file("image.png", "original binary content");
        f.write_patch(
            "\
diff --git a/image.png b/image.png
index 1234567..89abcdef 100644
Binary files a/image.png and b/image.png differ
",
        );

        f.apply().unwrap();
        assert_data_eq!(f.read_file("image.png"), str!["original binary content"]);
    }

    #[test]
    fn text_patch_can_apply_non_utf8_content() {
        let f = Fixture::new();

        f.write_file_bytes("file.bin", b"\xff\n");
        f.write_patch_bytes(b"diff --git a/file.bin b/file.bin\n--- a/file.bin\n+++ b/file.bin\n@@ -1 +1 @@\n-\xff\n+\xfe\n");

        f.apply().unwrap();
        assert_eq!(f.read_file_bytes("file.bin"), b"\xfe\n");
    }
}
