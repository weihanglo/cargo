//! Tests for unstable `patch-files` feature.

use crate::prelude::*;
use cargo_test_support::Project;
use cargo_test_support::basic_manifest;
use cargo_test_support::compare::assert_e2e;
use cargo_test_support::git;
use cargo_test_support::paths;
use cargo_test_support::prelude::*;
use cargo_test_support::project;
use cargo_test_support::registry;
use cargo_test_support::registry::Package;
use cargo_test_support::str;

const HELLO_PATCH: &str = r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+pub fn hello() {
+    println!("Hello, patched!")
+}
"#;

/// Helper to create a package with a patch.
fn patched_project() -> Project {
    Package::new("bar", "1.0.0").publish();
    project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .build()
}

#[cargo_test]
fn gated_manifest() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_status(101)
        .with_stderr_data(str![[r#"
[WARNING] ignoring `patches` on patch for `bar` in `https://github.com/rust-lang/crates.io-index`: see https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#patch-files about the status of this feature.
[UPDATING] `dummy-registry` index
[ERROR] patch for `bar` points to the same source, but patches must point to different sources
[HELP] check `bar` patch definition for `https://github.com/rust-lang/crates.io-index` in `[ROOT]/foo/Cargo.toml`

"#]])
        .run();
}

#[cargo_test]
fn gated_config() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .file(
            ".cargo/config.toml",
            r#"
                [patch.crates-io]
                bar = { version = "1.0.0", patches = [] }
            "#,
        )
        .build();

    p.cargo("check")
        .with_status(101)
        .with_stderr_data(str![[r#"
[WARNING] ignoring `patches` on patch for `bar` in `https://github.com/rust-lang/crates.io-index`: see https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#patch-files about the status of this feature.
[WARNING] [patch] in cargo config: ignoring `patches` on patch for `bar` in `https://github.com/rust-lang/crates.io-index`: see https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#patch-files about the status of this feature.
[UPDATING] `dummy-registry` index
[ERROR] patch for `bar` points to the same source, but patches must point to different sources
[HELP] check `bar` patch definition for `https://github.com/rust-lang/crates.io-index` in `[ROOT]/foo/.cargo/config.toml`

"#]])
        .run();
}

#[cargo_test]
fn warn_if_in_normal_dep() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = { version = "1", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[LOCKING] 1 package to latest compatible version
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[CHECKING] bar v1.0.0
[CHECKING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

#[cargo_test]
fn disallow_non_exact_version() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  patch for `bar` in `https://github.com/rust-lang/crates.io-index` requires at least one patch file when patching with files

"#]])
        .run();
}

#[cargo_test]
fn disallow_empty_patches_array() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  patch for `bar` in `https://github.com/rust-lang/crates.io-index` requires at least one patch file when patching with files

"#]])
        .run();
}

#[cargo_test]
fn disallow_mismatched_source_url() {
    registry::alt_init();
    Package::new("bar", "1.0.0").alternative(true).publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", registry = "alternative", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  patch for `bar` in `https://github.com/rust-lang/crates.io-index` requires at least one patch file when patching with files

"#]])
        .run();
}

#[cargo_test]
fn disallow_path_dep() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { path = "bar", patches = ["test.patch"] }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("bar/src/lib.rs", "")
        .file("test.patch", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  patch for `bar` in `https://github.com/rust-lang/crates.io-index` cannot use `patches` with a path dependency
  [HELP] apply the patch to the source directly, or copy the source to a separate directory

"#]])
        .run();
}

#[cargo_test]
fn disallow_git_dep() {
    let git = git::repo(&paths::root().join("bar"))
        .file("Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("src/lib.rs", "")
        .build();
    let url = git.url();

    let p = project()
        .file(
            "Cargo.toml",
            &format!(
                r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = {{ git = "{url}", patches = [""] }}
                "#
            ),
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  failed to checksum [ROOT]/foo

Caused by:
  failed to read `[ROOT]/foo`

Caused by:
  Is a directory (os error 21)

"#]])
        .run();
}

#[cargo_test]
fn patch() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch 46806b94)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();

    let actual = p.read_lockfile();
    let expected = str![[r##"
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "bar"
version = "1.0.0"
source = "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c"

[[package]]
name = "foo"
version = "0.0.0"
dependencies = [
 "bar",
]

"##]];
    assert_e2e().eq(actual, expected);
}

#[cargo_test]
fn patch_from_subdirectory() {
    // Test that running cargo from a subdirectory still finds patch files
    // relative to the workspace root, not cwd.
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [workspace]
                members = ["member"]

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file(
            "member/Cargo.toml",
            r#"
                [package]
                name = "member"
                edition = "2015"

                [dependencies]
                bar = "1"
            "#,
        )
        .file("member/src/main.rs", "fn main() { bar::hello(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    // Run from the "member" subdirectory
    p.cargo("run")
        .cwd(p.root().join("member"))
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch 46806b94)
[COMPILING] member v0.0.0 ([ROOT]/foo/member)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `[ROOT]/foo/target/debug/member`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();
}

#[cargo_test]
fn patch_in_config() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file(
            ".cargo/config.toml",
            r#"
                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("run -Zpatch-files")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch 46806b94)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();
}

#[cargo_test]
fn patch_for_alternative_registry() {
    registry::alt_init();
    Package::new("bar", "1.0.0").alternative(true).publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = { version = "1", registry = "alternative" }

                [patch.alternative]
                bar = { version = "1.0.0", registry = "alternative", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `alternative` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `alternative`)
[PATCHING] bar v1.0.0 (registry `alternative`)
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from alternative with patch 46806b94)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();
}

#[cargo_test]
fn patch_transitive_dep() {
    // Publish bar which depends on baz
    Package::new("baz", "1.0.0")
        .file("src/lib.rs", "pub fn baz() -> u32 { 1 }")
        .publish();
    Package::new("baz", "2.0.0")
        .file("src/lib.rs", "pub fn baz() -> u32 { 2 }")
        .publish();
    Package::new("bar", "1.0.0")
        .dep("baz", "1.0")
        .file("src/lib.rs", "pub fn baz() -> u32 { baz::baz() }")
        .publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file(
            "src/main.rs",
            r#"fn main() { println!("{}", bar::baz()); }"#,
        )
        .file(
            "patches/hello.patch",
            "--- a/Cargo.toml
+++ b/Cargo.toml
@@ -7,2 +7,2 @@
                 [dependencies.baz]
-                version = \"1.0\"
+                version = \"2.0\"
",
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![[r#"
2

"#]])
        .run();
}

#[cargo_test]
fn patch_package_version() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2021"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/lib.rs", "use bar::explode;")
        .file(
            "patches/hello.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -3,5 +3,5 @@

             [package]
             name = "bar"
-            version = "1.0.0"
+            version = "2.0.0"
             authors = []
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -3,0 +4,1 @@
+pub fn explode() {}
"#,
        )
        .build();

    // Build fails because patches are applied during dependency query.
    // After patching bar version to `2.0.0` ,
    // the cargo cannot find a version that matches the original `1.0.0` requirement.
    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[WARNING] patch `bar v2.0.0 (from crates-io with patch e84fb415)` was not used in the crate graph
[HELP] Check that the patched package version and available features are compatible
      with the dependency requirements. If the patch has a different version from
      what is locked in the Cargo.lock file, run `cargo update` to use the new
      version. This may also occur with an optional dependency that is not enabled.
[LOCKING] 1 package to latest compatible version
[ADDING] bar v1.0.0 (available: v2.0.0)
[CHECKING] bar v1.0.0
[CHECKING] foo v0.0.0 ([ROOT]/foo)
...
error[E0432]: unresolved import `bar::explode`
...
[ERROR] could not compile `foo` (lib) due to 1 previous error

"#]])
        .run();
}

#[cargo_test]
fn multiple_patches() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io.bar]
                version = "1.0.0"
                patches = ["patches/hello.patch", "../hola.patch"]
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); bar::hola(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .file(
            "../hola.patch",
            r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -3,0 +4,3 @@
+pub fn hola() {
+    println!("¡Hola, patched!")
+}
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch 999bb70f)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!
¡Hola, patched!

"#]])
        .run();

    let actual = p.read_lockfile();
    let expected = str![[r##"
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "bar"
version = "1.0.0"
source = "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=999bb70f0e374dc7c713e0c2a44147442f96ab022b9bc898235f2fd6cbd7b66d"

[[package]]
name = "foo"
version = "0.0.0"
dependencies = [
 "bar",
]

"##]];
    assert_e2e().eq(actual, expected);
}

#[cargo_test]
fn patch_nonexistent_patch() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  failed to checksum [ROOT]/foo/patches/hello.patch

Caused by:
  failed to open file `[ROOT]/foo/patches/hello.patch`

Caused by:
  [NOT_FOUND]

"#]])
        .run();
}

#[cargo_test]
fn no_rebuild_if_no_patch_changed() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch 46806b94)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();

    p.cargo("run -v")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[FRESH] bar v1.0.0 (from crates-io with patch 46806b94)
[FRESH] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo[EXE]`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();
}

#[cargo_test]
fn rebuild_if_patch_changed() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch 46806b94)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello, patched!

"#]])
        .run();

    p.change_file(
        "patches/hello.patch",
        r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+pub fn hello() {
+    println!("¡Hola, patched!")
+}
"#,
    );

    // Patch content changed, checksum changed. Cargo detects mismatch and re-resolves.
    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[ADDING] bar v1.0.0 (from crates-io with patch 88122499)
[COMPILING] bar v1.0.0 (from crates-io with patch 88122499)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
¡Hola, patched!

"#]])
        .run();
}

#[cargo_test]
fn cargo_pkgid() {
    let p = patched_project();

    p.cargo("generate-lockfile")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version

"#]])
        .run();

    p.cargo("pkgid bar")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![[r#"
patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c#bar@1.0.0

"#]])
        .run();
}

#[cargo_test]
fn track_unused_in_lockfile() {
    Package::new("bar", "1.0.0").publish();
    Package::new("bar", "2.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "2"

                [patch.crates-io]
                bar = { version = "1", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() {}")
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[WARNING] patch `bar v1.0.0 (from crates-io with patch 46806b94)` was not used in the crate graph
[HELP] Check that the patched package version and available features are compatible
      with the dependency requirements. If the patch has a different version from
      what is locked in the Cargo.lock file, run `cargo update` to use the new
      version. This may also occur with an optional dependency that is not enabled.
[LOCKING] 1 package to latest compatible version
[DOWNLOADING] crates ...
[DOWNLOADED] bar v2.0.0 (registry `dummy-registry`)
[COMPILING] bar v2.0.0
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo[EXE]`

"#]])
        .run();

    let actual = p.read_lockfile();
    let expected = str![[r##"
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "bar"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a184cee92224be6149c9e218327188d1d74a4514f971b1e3ce0170ea94ea5da7"

[[package]]
name = "foo"
version = "0.0.0"
dependencies = [
 "bar",
]

[[patch.unused]]
name = "bar"
version = "1.0.0"
source = "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c"

"##]];
    assert_e2e().eq(actual, expected);
}

#[cargo_test]
fn cargo_metadata() {
    let p = patched_project();

    p.cargo("generate-lockfile")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version

"#]])
        .run();

    p.cargo("metadata")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![[r#"
{
  "...": "{...}",
  "packages": [
    {
      "...": "{...}",
      "id": "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c#bar@1.0.0",
      "manifest_path": "[ROOT]/home/.cargo/patched-src/github.com-[HASH]/bar-1.0.0/46806b94/Cargo.toml",
      "source": "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c",
      "targets": [
        {
          "...": "{...}",
          "src_path": "[ROOT]/home/.cargo/patched-src/github.com-[HASH]/bar-1.0.0/46806b94/src/lib.rs"
        }
      ]
    },
    "{...}"
  ],
  "resolve": {
    "...": "{...}",
    "nodes": [
      {
        "...": "{...}",
        "id": "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c#bar@1.0.0"
      },
      {
        "...": "{...}",
        "dependencies": [
          "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c#bar@1.0.0"
        ],
        "deps": [
          {
            "...": "{...}",
            "pkg": "patched+registry+https://github.com/rust-lang/crates.io-index?patch-cksum=46806b943777e31efd3c0708a98bb6b19d369d3036766ef2b2f27d7c236ff68c#bar@1.0.0"
          }
        ]
      }
    ]
  }
}
"#]]
            .is_json(),
)
        .run();
}

#[cargo_test]
fn empty_patch_file_error() {
    Package::new("bar", "1.0.0").publish();
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/empty.patch"] }
            "#,
        )
        .file("src/lib.rs", "")
        .file("patches/empty.patch", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[ERROR] no valid patches found in `[ROOT]/foo/patches/empty.patch`

"#]])
        .run();
}

#[cargo_test]
fn git_format_patch_output() {
    // Test that patches generated by `git format-patch` work correctly,
    // including the RFC 3676 email signature stripping.
    Package::new("bar", "1.0.0").publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = "1"

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/fix.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        // This is the format produced by `git format-patch`
        .file(
            "patches/fix.patch",
            r#"From 1234567890abcdef1234567890abcdef12345678 Mon Sep 17 00:00:00 2001
From: Gandalf <gandalf@the.grey>
Date: Mon, 25 Mar 3019 00:00:00 +0000
Subject: [PATCH] fix!: destroy the one ring at mount doom

In a hole in the ground there lived a hobbit
---
 src/lib.rs | 3 +++
 1 file changed, 3 insertions(+)

--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+pub fn hello() {
+    println!("The ring is destroyed!");
+}
-- 
2.40.0
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from crates-io with patch f315317f)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
The ring is destroyed!

"#]])
        .run();
}

#[cargo_test]
fn patch_git_source() {
    // Test that patching a git source works.
    let git_project = git::repo(&paths::root().join("bar"))
        .file("Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("src/lib.rs", "// original\n")
        .build();
    let url = git_project.url();

    let p = project()
        .file(
            "Cargo.toml",
            &format!(
                r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2015"

                [dependencies]
                bar = {{ git = "{url}" }}

                [patch."{url}"]
                bar = {{ git = "{url}", patches = ["patches/hello.patch"] }}
                "#
            ),
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file(
            "patches/hello.patch",
            r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1,3 @@
-// original
+pub fn hello() {
+    println!("Hello from patched git!");
+}
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[UPDATING] git repository `[ROOTURL]/bar`
[PATCHING] bar v1.0.0 ([ROOTURL]/bar#[..])
[UPDATING] git repository `[ROOTURL]/bar`
[LOCKING] 1 package to latest compatible version
[COMPILING] bar v1.0.0 (from git+[ROOTURL]/bar with patch b0d23d81)
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[RUNNING] `target/debug/foo`

"#]])
        .with_stdout_data(str![[r#"
Hello from patched git!

"#]])
        .run();
}

#[cargo_test]
fn patch_git_workspace_inheritance() {
    // Test that patching a git monorepo with workspace inheritance works.
    // This requires copying the entire repo, not just the package subdirectory.
    let git_project = git::repo(&paths::root().join("my-workspace"))
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = ["bar"]

                [workspace.package]
                version = "1.0.0"
                edition = "2021"
            "#,
        )
        .file(
            "bar/Cargo.toml",
            r#"
                [package]
                name = "bar"
                version.workspace = true
                edition.workspace = true
            "#,
        )
        .file("bar/src/lib.rs", "// original\n")
        .build();
    let url = git_project.url();

    let p = project()
        .file(
            "Cargo.toml",
            &format!(
                r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2021"

                [dependencies]
                bar = {{ git = "{url}" }}

                [patch."{url}"]
                bar = {{ git = "{url}", patches = ["patches/hello.patch"] }}
                "#
            ),
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file(
            "patches/hello.patch",
            r#"
--- a/bar/src/lib.rs
+++ b/bar/src/lib.rs
@@ -1 +1,3 @@
-// original
+pub fn hello() {
+    println!("Hello from patched git workspace!");
+}
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![[r#"
Hello from patched git workspace!

"#]])
        .run();
}

#[cargo_test]
fn patch_git_same_patches_reused() {
    // Test that using the same patches for multiple packages from the same git repo
    // reuses the same patched directory (per-repo, not per-package).
    let git_project = git::repo(&paths::root().join("my-workspace"))
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = ["bar", "baz"]
            "#,
        )
        .file("bar/Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("bar/src/lib.rs", "pub fn bar() {}\n")
        .file("baz/Cargo.toml", &basic_manifest("baz", "1.0.0"))
        .file("baz/src/lib.rs", "pub fn baz() {}\n")
        .build();
    let url = git_project.url();

    let p = project()
        .file(
            "Cargo.toml",
            &format!(
                r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2021"

                [dependencies]
                bar = {{ git = "{url}" }}
                baz = {{ git = "{url}" }}

                [patch."{url}"]
                bar = {{ git = "{url}", patches = ["patches/shared.patch"] }}
                baz = {{ git = "{url}", patches = ["patches/shared.patch"] }}
                "#
            ),
        )
        .file(
            "src/lib.rs",
            "use bar::bar; use baz::baz; pub fn foo() { bar(); baz(); }",
        )
        .file(
            "patches/shared.patch",
            r#"
--- a/bar/src/lib.rs
+++ b/bar/src/lib.rs
@@ -1 +1 @@
-pub fn bar() {}
+pub fn bar() { /* patched */ }
--- a/baz/src/lib.rs
+++ b/baz/src/lib.rs
@@ -1 +1 @@
-pub fn baz() {}
+pub fn baz() { /* patched */ }
"#,
        )
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(
            str![[r#"
[UPDATING] git repository `[ROOTURL]/my-workspace`
[PATCHING] bar v1.0.0 ([ROOTURL]/my-workspace#[..])
[UPDATING] git repository `[ROOTURL]/my-workspace`
[LOCKING] 2 packages to latest compatible versions
[CHECKING] foo v0.0.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s
[CHECKING] baz v1.0.0 (from git+[ROOTURL]/my-workspace with patch 0f89441a)
[CHECKING] bar v1.0.0 (from git+[ROOTURL]/my-workspace with patch 0f89441a)

"#]]
            .unordered(),
        )
        .run();
}

#[cargo_test]
fn patch_git_conflicting_patches_error() {
    // Test that using different patches for packages from the same git repo errors out.
    // This is because we do per-repo copying, not per-package.
    let git_project = git::repo(&paths::root().join("my-workspace"))
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = ["bar", "baz"]
            "#,
        )
        .file("bar/Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("bar/src/lib.rs", "")
        .file("baz/Cargo.toml", &basic_manifest("baz", "1.0.0"))
        .file("baz/src/lib.rs", "")
        .build();
    let url = git_project.url();

    let p = project()
        .file(
            "Cargo.toml",
            &format!(
                r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                edition = "2021"

                [dependencies]
                bar = {{ git = "{url}" }}
                baz = {{ git = "{url}" }}

                [patch."{url}"]
                bar = {{ git = "{url}", patches = ["patches/1.patch"] }}
                baz = {{ git = "{url}", patches = ["patches/2.patch"] }}
                "#
            ),
        )
        .file("src/lib.rs", "")
        // Patch files must have different content to produce different checksums.
        // Content doesn't need to be valid patches since we're testing conflict detection.
        .file("patches/1.patch", "patch content 1")
        .file("patches/2.patch", "patch content 2")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] conflicting patch files for git repository `[ROOTURL]/my-workspace`
`bar` uses patches: ["[ROOT]/foo/patches/1.patch"]
`baz` uses patches: ["[ROOT]/foo/patches/2.patch"]
[HELP] all packages from the same git repository must use identical patch files

"#]])
        .run();
}
