//! Utilities for applying unified diff patches.

use std::path::Path;

use cargo_util::paths;
use diffy::FileOperation;

use crate::CargoResult;

/// Applies a [unified diff] format patch file to a directory.
///
/// Currently support:
///
/// * multiple file patches
/// * modify files
/// * rename files
/// * create files (`--- /dev/null`)
/// * delete files (`+++ /dev/null`)
/// * line endings normalized to LF
/// * strip first path component (like `patch -p1`)
/// * strip trailing [RFC 3676] email signature (usually added by [`git format-patch`])
///
/// [unified diff]: https://www.gnu.org/software/diffutils/manual/html_node/Unified-Format.html
/// [RFC 3676]: https://www.rfc-editor.org/rfc/rfc3676#section-4.3
/// [`git format-patch`]: https://git-scm.com/docs/git-format-patch#Documentation/git-format-patch.txt---no-signature
pub fn apply_patch_file(patch_file: &Path, dst: &Path) -> CargoResult<()> {
    let patch_content = paths::read(patch_file)?;
    // Normalize CRLF to LF since diffy doesn't handle CRLF.
    let patch_content = patch_content.replace("\r\n", "\n");

    let patchset = diffy::PatchSet::from_str(&patch_content)?;
    if patchset.is_empty() {
        anyhow::bail!("no valid patches found in `{}`", patch_file.display());
    }

    for file_patch in patchset.patches() {
        let op = file_patch.operation().strip_prefix(1);
        let patch = file_patch.patch();

        match op {
            FileOperation::Delete(path) => {
                paths::remove_file(dst.join(path))?;
            }
            FileOperation::Create(path) => {
                let target_path = dst.join(path);
                let patched = diffy::apply("", patch)?;
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                paths::write(&target_path, &patched)?;
            }
            FileOperation::Modify { from, to } => {
                let source_path = dst.join(&from);
                let target_path = dst.join(&to);
                let original = paths::read(&source_path)?;
                let patched = diffy::apply(&original, patch)?;
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                paths::write(&target_path, &patched)?;
                if from != to {
                    // If renamed, remove the original file
                    paths::remove_file(&source_path)?;
                }
            }
        }
    }

    Ok(())
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

        fn read_file(&self, name: &str) -> String {
            std::fs::read_to_string(self.dst.join(name)).unwrap()
        }

        fn file_exists(&self, name: &str) -> bool {
            self.dst.join(name).exists()
        }

        fn write_patch(&self, content: &str) {
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
    fn rename_file() {
        let f = Fixture::new();

        f.write_file("from.rs", "content\n");
        f.write_patch(
            "\
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
    fn multi_file_patch() {
        let f = Fixture::new();

        f.write_file("a.rs", "old1\n");
        f.write_file("b.rs", "old2\n");
        f.write_patch(
            "\
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old1
+new1
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
    fn crlf_normalization() {
        let f = Fixture::new();

        f.write_file("file.rs", "old\n");
        f.write_patch("--- a/file.rs\r\n+++ b/file.rs\r\n@@ -1 +1 @@\r\n-old\r\n+new\r\n");

        f.apply().unwrap();
        assert_data_eq!(
            f.read_file("file.rs"),
            str![[r#"
new

"#]]
        );
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
    fn git_format_patch_with_preamble() {
        let f = Fixture::new();

        f.write_file("file.rs", "old\n");
        f.write_patch(
            r#"From 1234567890abcdef1234567890abcdef12345678 Mon Sep 17 00:00:00 2001
From: Author <test@example.com>
Date: Mon, 1 Jan 2026 00:00:00 +0000
Subject: [PATCH] subject line

This is body
---
 file.rs | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

--- a/file.rs
+++ b/file.rs
@@ -1 +1 @@
-old
+new
-- 
2.40.0
"#,
        );

        f.apply().unwrap();
        assert_data_eq!(
            f.read_file("file.rs"),
            str![[r#"
new
"#]]
        );
    }
}
