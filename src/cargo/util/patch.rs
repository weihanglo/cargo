//! Utilities for applying unified diff patches.

use std::path::Path;
use std::path::PathBuf;

use cargo_util::paths;

use crate::CargoResult;

/// Prefix for the original file path (e.g., `--- a/file.rs`).
const ORIGINAL_PREFIX: &str = "--- ";
/// Prefix for the modified file path (e.g., `+++ b/file.rs`).
const MODIFIED_PREFIX: &str = "+++ ";
/// Prefix for a hunk header (e.g., `@@ -1,3 +1,4 @@`).
const HUNK_PREFIX: &str = "@@ ";
/// Path used to indicate file creation or deletion.
const DEV_NULL: &str = "/dev/null";

/// The operation to perform based on a patch.
#[derive(Debug, PartialEq, Eq)]
enum PatchOperation {
    /// Delete a file (`+++ /dev/null`).
    Delete(PathBuf),
    /// Create a new file (`--- /dev/null`).
    Create(PathBuf),
    /// Modify or rename a file.
    ///
    /// * `from == to` → modify file in place
    /// * `from != to` → read from `from`, write to `to`, delete `from`
    Modify { from: PathBuf, to: PathBuf },
}

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
    // Strip trailing email signature
    let patch_content = patch_content
        .rsplit_once("\n-- \n")
        .map(|(body, _signature)| body)
        .unwrap_or(&patch_content);

    let patches = split_patches(patch_content);
    if patches.is_empty() {
        anyhow::bail!("no valid patches found in `{}`", patch_file.display());
    }

    for file_patch in patches {
        let patch = diffy::Patch::from_str(file_patch)?;
        let op = extract_patch_operation(&patch)?;

        match op {
            PatchOperation::Delete(path) => {
                paths::remove_file(dst.join(path))?;
            }
            PatchOperation::Create(path) => {
                let target_path = dst.join(path);
                let patched = diffy::apply("", &patch)?;
                if let Some(parent) = target_path.parent() {
                    paths::create_dir_all(parent)?;
                }
                paths::write(&target_path, &patched)?;
            }
            PatchOperation::Modify { from, to } => {
                let source_path = dst.join(&from);
                let target_path = dst.join(&to);
                let original = paths::read(&source_path)?;
                let patched = diffy::apply(&original, &patch)?;
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

/// Splits a unified diff containing multiple file patches.
fn split_patches(content: &str) -> Vec<&str> {
    let mut patches = Vec::new();
    let mut patch_start = None::<usize>;
    let mut prev_line = None::<&str>;
    let mut byte_offset = 0;

    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let next_line = lines.peek().copied();

        if is_patch_boundary(prev_line, line, next_line) {
            if let Some(start) = patch_start {
                patches.push(&content[start..byte_offset]);
            }
            patch_start = Some(byte_offset);
        }

        prev_line = Some(line);
        byte_offset += line.len();

        if content[byte_offset..].starts_with("\r\n") {
            byte_offset += 2;
        } else if content[byte_offset..].starts_with('\n') {
            byte_offset += 1;
        }
    }

    if let Some(start) = patch_start {
        patches.push(&content[start..]);
    }

    patches
}

/// Checks if the current line is a patch boundary.
///
/// A patch boundary is one of:
///
/// - `--- ` followed by `+++ ` on the next line
/// - `+++ ` followed by `--- ` on the next line
/// - `--- ` followed by `@@ ` on the next line (missing `+++`)
/// - `+++ ` followed by `@@ ` on the next line (missing `---`)
fn is_patch_boundary(prev: Option<&str>, line: &str, next: Option<&str>) -> bool {
    if line.starts_with(ORIGINAL_PREFIX) {
        // Make sure it isn't part of a (`+++` / `--- `) pair
        if prev.is_some_and(|p| p.starts_with(MODIFIED_PREFIX)) {
            return false;
        }
        // `--- ` followed by `+++ `
        if next.is_some_and(|n| n.starts_with(MODIFIED_PREFIX)) {
            return true;
        }
        // `--- ` followed by `@@ `
        if next.is_some_and(|n| n.starts_with(HUNK_PREFIX)) {
            return true;
        }
    }

    if line.starts_with(MODIFIED_PREFIX) {
        // Make sure it isn't part of a (`---` / `+++`) pair
        if prev.is_some_and(|p| p.starts_with(ORIGINAL_PREFIX)) {
            return false;
        }
        // `+++ ` followed by `--- `
        if next.is_some_and(|n| n.starts_with(ORIGINAL_PREFIX)) {
            return true;
        }
        // `+++ ` followed by `@@ `
        if next.is_some_and(|n| n.starts_with(HUNK_PREFIX)) {
            return true;
        }
    }

    false
}

/// Extracts the operation and target file path from a patch.
fn extract_patch_operation(patch: &diffy::Patch<'_, str>) -> CargoResult<PatchOperation> {
    let original = patch.original();
    let modified = patch.modified();

    let is_create = original == Some(DEV_NULL);
    let is_delete = modified == Some(DEV_NULL);

    if is_create && is_delete {
        anyhow::bail!("patch has both original and modified as /dev/null");
    }

    // Strip first path component like `patch -p1`.
    // For example, `a/src/lib.rs` becomes `src/lib.rs`.
    let strip_prefix = |path: &str| -> PathBuf {
        match path.split_once('/') {
            Some((_first, rest)) => PathBuf::from(rest),
            None => PathBuf::from(path),
        }
    };

    if is_delete {
        let path = original.ok_or_else(|| anyhow::anyhow!("delete patch has no original path"))?;
        Ok(PatchOperation::Delete(strip_prefix(path)))
    } else if is_create {
        let path = modified.ok_or_else(|| anyhow::anyhow!("create patch has no modified path"))?;
        Ok(PatchOperation::Create(strip_prefix(path)))
    } else {
        match (original, modified) {
            (Some(from), Some(to)) => {
                let from = strip_prefix(from);
                let to = strip_prefix(to);
                Ok(PatchOperation::Modify { from, to })
            }
            (None, Some(to)) => {
                // No original path, but has modified path.
                // This is a modify operation (not create) - GNU patch reads from the modified path.
                let path = strip_prefix(to);
                Ok(PatchOperation::Modify {
                    from: path.clone(),
                    to: path,
                })
            }
            (Some(from), None) => {
                let path = strip_prefix(from);
                Ok(PatchOperation::Modify {
                    from: path.clone(),
                    to: path,
                })
            }
            (None, None) => anyhow::bail!("patch has no file path"),
        }
    }
}

#[cfg(test)]
mod tests_extract_patch_operation {
    use super::*;

    #[test]
    fn modify() {
        let patch = diffy::Patch::from_str(
            "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(
            op,
            PatchOperation::Modify {
                from: PathBuf::from("src/lib.rs"),
                to: PathBuf::from("src/lib.rs"),
            }
        );
    }

    #[test]
    fn new_file() {
        let patch = diffy::Patch::from_str(
            "\
--- /dev/null
+++ b/src/lib.rs
@@ -0,0 +1 @@
+content
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(op, PatchOperation::Create(PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn delete_file() {
        let patch = diffy::Patch::from_str(
            "\
--- a/src/lib.rs
+++ /dev/null
@@ -1 +0,0 @@
-content
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(op, PatchOperation::Delete(PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn rename() {
        // When original and modified paths differ, it's a rename.
        let patch = diffy::Patch::from_str(
            "\
--- a/old_name.rs
+++ b/new_name.rs
@@ -1,3 +1,3 @@
 line1
-line2
+modified
 line3
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(
            op,
            PatchOperation::Modify {
                from: PathBuf::from("old_name.rs"),
                to: PathBuf::from("new_name.rs"),
            }
        );
    }

    #[test]
    fn custom_prefix() {
        // Non-standard prefixes should still work
        let patch = diffy::Patch::from_str(
            "\
--- old/src/lib.rs
+++ new/src/lib.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(
            op,
            PatchOperation::Modify {
                from: PathBuf::from("src/lib.rs"),
                to: PathBuf::from("src/lib.rs"),
            }
        );
    }

    #[test]
    fn missing_modified_uses_to_original() {
        // diffy can parse patches with missing +++ line.
        // GNU patch handles this the same way.
        let patch = diffy::Patch::from_str(
            "\
--- a/src/lib.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(
            op,
            PatchOperation::Modify {
                from: PathBuf::from("src/lib.rs"),
                to: PathBuf::from("src/lib.rs"),
            }
        );
    }

    #[test]
    fn missing_original_uses_modified() {
        // diffy can parse patches with missing --- line
        // GNU patch handles this the same way:
        let patch = diffy::Patch::from_str(
            "\
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(
            op,
            PatchOperation::Modify {
                from: PathBuf::from("src/lib.rs"),
                to: PathBuf::from("src/lib.rs"),
            }
        );
    }

    #[test]
    fn both_dev_null_errors() {
        // GNU patch rejects this: "can't find file to patch"
        let patch = diffy::Patch::from_str(
            "\
--- /dev/null
+++ /dev/null
@@ -1 +0,0 @@
-old
",
        )
        .unwrap();
        let result = extract_patch_operation(&patch);
        assert!(result.is_err());
    }

    #[test]
    fn reversed_header_order() {
        // +++ before --- is non-standard but GNU patch accepts it.
        // diffy also accepts it, treating +++ as modified and --- as original.
        //
        // ```console
        // $ echo "old" > file.rs
        // $ cat > reversed.patch << 'EOF'
        // +++ b/file.rs
        // --- a/file.rs
        // @@ -1 +1 @@
        // -old
        // +new
        // EOF
        // $ patch -p1 < reversed.patch
        // patching file file.rs
        // ```
        let patch = diffy::Patch::from_str(
            "\
+++ b/file.rs
--- a/file.rs
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        // diffy swaps them: original is from ---, modified is from +++
        assert_eq!(patch.original(), Some("a/file.rs"));
        assert_eq!(patch.modified(), Some("b/file.rs"));
        let op = extract_patch_operation(&patch).unwrap();
        assert_eq!(
            op,
            PatchOperation::Modify {
                from: PathBuf::from("file.rs"),
                to: PathBuf::from("file.rs"),
            }
        );
    }

    #[test]
    fn missing_both_paths_errors() {
        let patch = diffy::Patch::from_str(
            "\
@@ -1 +1 @@
-old
+new
",
        )
        .unwrap();
        let result = extract_patch_operation(&patch);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod tests_split_patches {
    use super::*;
    use snapbox::assert_data_eq;
    use snapbox::str;

    #[test]
    fn single_file_patch() {
        let content = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,4 @@
 line1
 line2
+line3
 line4
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,4 @@
 line1
 line2
+line3
 line4

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn multi_file_patch() {
        let content = "\
--- a/file1.rs
+++ b/file1.rs
@@ -1 +1 @@
-old1
+new1
--- a/file2.rs
+++ b/file2.rs
@@ -1 +1 @@
-old2
+new2
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 2);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- a/file1.rs
+++ b/file1.rs
@@ -1 +1 @@
-old1
+new1

"#]]
        );
        assert_data_eq!(
            patches[1],
            str![[r#"
--- a/file2.rs
+++ b/file2.rs
@@ -1 +1 @@
-old2
+new2

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
        assert!(diffy::Patch::from_str(patches[1]).is_ok());
    }

    #[test]
    fn patch_with_preamble() {
        let content = "\
This is a preamble
It should be ignored
--- a/file.rs
+++ b/file.rs
@@ -1 +1 @@
-old
+new
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- a/file.rs
+++ b/file.rs
@@ -1 +1 @@
-old
+new

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn ignores_false_positive() {
        // line starting with "--- " but not a patch boundary
        let content = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 line1
---- this is not a patch boundary
+--- this line starts with dashes
 line3
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn split_empty_content() {
        let patches = split_patches("");
        assert!(patches.is_empty());
    }

    #[test]
    fn git_format_patch() {
        let content = "\
From 1234567890abcdef1234567890abcdef12345678 Mon Sep 17 00:00:00 2001
From: Gandalf <gandarf@the.grey>
Date: Mon, 25 Mar 3019 00:00:00 +0000
Subject: [PATCH] fix!: destroy the one ring at mount doom

In a hole in the ground there lived a hobbit
---
 src/frodo.rs | 2 +-
 src/sam.rs   | 1 +
 2 files changed, 2 insertions(+), 1 deletion(-)

--- a/src/frodo.rs
+++ b/src/frodo.rs
@@ -1 +1 @@
-finger
+peace
--- a/src/sam.rs
+++ b/src/sam.rs
@@ -1 +1,2 @@
 food
+more food
-- 
2.40.0
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 2);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- a/src/frodo.rs
+++ b/src/frodo.rs
@@ -1 +1 @@
-finger
+peace

"#]]
        );
        assert_data_eq!(
            patches[1],
            str![[r#"
--- a/src/sam.rs
+++ b/src/sam.rs
@@ -1 +1,2 @@
 food
+more food
-- 
2.40.0

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
        // Last patch has trailing content `--\n2.40.0` that diffy rejects
        assert!(diffy::Patch::from_str(patches[1]).is_err());
    }

    #[test]
    fn crlf() {
        let content = "--- a/file1.rs\r\n+++ b/file1.rs\r\n@@ -1 +1 @@\r\n-old1\r\n+new1\r\n--- a/file2.rs\r\n+++ b/file2.rs\r\n@@ -1 +1 @@\r\n-old2\r\n+new2\r\n";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 2);
        assert_data_eq!(
            patches[0],
            "--- a/file1.rs\r\n+++ b/file1.rs\r\n@@ -1 +1 @@\r\n-old1\r\n+new1\r\n"
        );
        assert_data_eq!(
            patches[1],
            "--- a/file2.rs\r\n+++ b/file2.rs\r\n@@ -1 +1 @@\r\n-old2\r\n+new2\r\n"
        );
        // diffy rejects CRLF line endings (unlike GNU patch).
        assert!(diffy::Patch::from_str(patches[0]).is_err());
        assert!(diffy::Patch::from_str(patches[1]).is_err());
    }

    #[test]
    fn no_newline_at_eof() {
        let content = "\
--- a/file.rs
+++ b/file.rs
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert!(patches[0].contains("No newline at end of file"));
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn binary_file_marker() {
        // Git outputs "Binary files ... differ" for binary files.
        let content = "\
--- /dev/null
+++ b/file.txt
@@ -0,0 +1 @@
+text
Binary files a/image.png and b/image.png differ
";
        let patches = split_patches(content);
        // binary marker is not a patch (yet)
        assert_eq!(patches.len(), 1);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- /dev/null
+++ b/file.txt
@@ -0,0 +1 @@
+text
Binary files a/image.png and b/image.png differ

"#]]
        );
        // diffy rejects the trailing "Binary files" line
        assert!(diffy::Patch::from_str(patches[0]).is_err());
    }

    #[test]
    fn not_a_patch() {
        let content = "Some random text\nNo patches here\n";
        let patches = split_patches(content);
        assert!(patches.is_empty());
    }

    #[test]
    fn incomplete_header() {
        // Has --- but no following +++ or @@
        let content = "\
--- a/file.rs
Some random text
No patches here
";
        let patches = split_patches(content);
        assert!(patches.is_empty());
    }

    #[test]
    fn missing_modified_header() {
        let content = "\
--- a/file.rs
@@ -1 +1 @@
-old
+new
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- a/file.rs
@@ -1 +1 @@
-old
+new

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn missing_original_header() {
        let content = "\
+++ b/file.rs
@@ -1 +1 @@
-old
+new
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert_data_eq!(
            patches[0],
            str![[r#"
+++ b/file.rs
@@ -1 +1 @@
-old
+new

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn reversed_header_order() {
        let content = "\
+++ b/file.rs
--- a/file.rs
@@ -1 +1 @@
-old
+new
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 1);
        assert_data_eq!(
            patches[0],
            str![[r#"
+++ b/file.rs
--- a/file.rs
@@ -1 +1 @@
-old
+new

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
    }

    #[test]
    fn multi_file_mixed_headers() {
        let content = "\
--- a/file1.rs
+++ b/file1.rs
@@ -1 +1 @@
-old1
+new1
--- a/file2.rs
@@ -1 +1 @@
-old2
+new2
+++ b/file3.rs
@@ -1 +1 @@
-old3
+new3
";
        let patches = split_patches(content);
        assert_eq!(patches.len(), 3);
        assert_data_eq!(
            patches[0],
            str![[r#"
--- a/file1.rs
+++ b/file1.rs
@@ -1 +1 @@
-old1
+new1

"#]]
        );
        assert_data_eq!(
            patches[1],
            str![[r#"
--- a/file2.rs
@@ -1 +1 @@
-old2
+new2

"#]]
        );
        assert_data_eq!(
            patches[2],
            str![[r#"
+++ b/file3.rs
@@ -1 +1 @@
-old3
+new3

"#]]
        );
        assert!(diffy::Patch::from_str(patches[0]).is_ok());
        assert!(diffy::Patch::from_str(patches[1]).is_ok());
        assert!(diffy::Patch::from_str(patches[2]).is_ok());
    }
}

#[cfg(test)]
mod tests_apply_patch_file {
    use super::*;
    use snapbox::assert_data_eq;
    use snapbox::str;

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
    fn signature_separator_stripped() {
        let f = Fixture::new();

        f.write_file("file.rs", "old\n");
        // Preamble contains "-- " lines that look like signature separators.
        f.write_patch(
            r#"preamble start
-- 
-- 
-- 
-- 
preamble end
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
