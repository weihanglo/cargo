//! Tests for unstable `patch-files` feature.

use crate::prelude::*;
use cargo_test_support::Project;
use cargo_test_support::basic_manifest;
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
[WARNING] unused manifest key: patch.crates-io.bar.patches
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
[WARNING] unused manifest key: patch.crates-io.bar.patches
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
[WARNING] unused manifest key: dependencies.bar.patches
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
    Package::new("bar", "1.1.0").publish();
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
                bar = { version = "1", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/lib.rs", "")
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

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
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

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
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
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
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
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
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
        .run();
}

#[cargo_test]
fn patch() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
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
[ERROR] failed searching for potential workspace
package manifest: `[ROOT]/foo/member/Cargo.toml`
invalid potential workspace manifest: `[ROOT]/foo/Cargo.toml`

[HELP] to avoid searching for a non-existent workspace, add `[workspace]` to the package manifest

Caused by:
  failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] unknown `-Z` flag specified: patch-files

For available unstable features, see https://doc.rust-lang.org/nightly/cargo/reference/unstable.html
If you intended to use an unstable rustc feature, try setting `RUSTFLAGS="-Zpatch-files"`

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
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
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn patch_cargo_toml_adds_feature() {
    Package::new("bar", "1.0.0")
        .file(
            "src/lib.rs",
            r#"
#[cfg(feature = "patched")]
pub fn hello() {
    println!("Hello, feature patched!");
}
"#,
        )
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
                bar = { version = "1", features = ["patched"] }

                [patch.crates-io]
                bar = { version = "1.0.0", patches = ["patches/features.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file(
            "patches/features.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -3,5 +3,8 @@

             [package]
             name = "bar"
             version = "1.0.0"
             authors = []
+
+[features]
+patched = []
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn patch_cargo_toml_adds_dependency() {
    Package::new("baz", "1.0.0")
        .file(
            "src/lib.rs",
            r#"
pub fn hello() {
    println!("Hello from baz!");
}
"#,
        )
        .publish();
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
                bar = { version = "1.0.0", patches = ["patches/dependency.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file(
            "patches/dependency.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -3,5 +3,8 @@

             [package]
             name = "bar"
             version = "1.0.0"
             authors = []
+
+[dependencies]
+baz = "1"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+pub fn hello() {
+    baz::hello();
+}
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn patch_cargo_toml_lowers_rust_version_for_resolution() {
    Package::new("lower-msrv", "1.0.0")
        .rust_version("1.80.0")
        .file(
            "src/lib.rs",
            r#"pub fn version() -> &'static str { "lower-msrv 1.0.0" }"#,
        )
        .publish();
    Package::new("lower-msrv", "1.1.0")
        .rust_version("1.999.0")
        .file(
            "src/lib.rs",
            r#"pub fn version() -> &'static str { "lower-msrv 1.1.0" }"#,
        )
        .publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"
                rust-version = "1.85.0"
                resolver = "3"

                [dependencies]
                lower-msrv = "1"
            "#,
        )
        .file(
            "src/main.rs",
            r#"
                fn main() {
                    println!("{}", lower_msrv::version());
                }
            "#,
        )
        .file(
            "patches/lower-msrv-10.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1 +1 @@
-
+# patched but still lower version
"#,
        )
        .file(
            "patches/lower-msrv-11.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -6 +6 @@
-        rust-version = "1.999.0"
+        rust-version = "1.80.0"
"#,
        )
        .build();

    p.cargo("run")
        .with_stdout_data(str![[r#"
lower-msrv 1.0.0

"#]])
        .run();

    p.change_file(
        "Cargo.toml",
        r#"
            cargo-features = ["patch-files"]

            [package]
            name = "foo"
            version = "0.0.1"
            edition = "2015"
            rust-version = "1.85.0"
            resolver = "3"

            [dependencies]
            lower-msrv = "1"

            [patch.crates-io]
            lower-msrv-10 = { package = "lower-msrv", version = "=1.0.0", patches = ["patches/lower-msrv-10.patch"] }
            lower-msrv-11 = { package = "lower-msrv", version = "=1.1.0", patches = ["patches/lower-msrv-11.patch"] }
        "#,
    );
    p.root().join("Cargo.lock").rm_rf();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn patch_cargo_toml_raises_rust_version_for_resolution() {
    Package::new("higher-msrv", "1.0.0")
        .rust_version("1.80.0")
        .file(
            "src/lib.rs",
            r#"pub fn version() -> &'static str { "higher-msrv 1.0.0" }"#,
        )
        .publish();
    Package::new("higher-msrv", "1.1.0")
        .rust_version("1.80.0")
        .file(
            "src/lib.rs",
            r#"pub fn version() -> &'static str { "higher-msrv 1.1.0" }"#,
        )
        .publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"
                rust-version = "1.85.0"
                resolver = "3"

                [dependencies]
                higher-msrv = "1"
            "#,
        )
        .file(
            "src/main.rs",
            r#"
                fn main() {
                    println!("{}", higher_msrv::version());
                }
            "#,
        )
        .file(
            "patches/higher-msrv-10.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1 +1 @@
-
+# patched but still lower version
"#,
        )
        .file(
            "patches/higher-msrv-11.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -6 +6 @@
-        rust-version = "1.80.0"
+        rust-version = "1.999.0"
"#,
        )
        .build();

    p.cargo("run")
        .with_stdout_data(str![[r#"
higher-msrv 1.1.0

"#]])
        .run();

    p.change_file(
        "Cargo.toml",
        r#"
            cargo-features = ["patch-files"]

            [package]
            name = "foo"
            version = "0.0.1"
            edition = "2015"
            rust-version = "1.85.0"
            resolver = "3"

            [dependencies]
            higher-msrv = "1"

            [patch.crates-io]
            higher-msrv-10 = { package = "higher-msrv", version = "=1.0.0", patches = ["patches/higher-msrv-10.patch"] }
            higher-msrv-11 = { package = "higher-msrv", version = "=1.1.0", patches = ["patches/higher-msrv-11.patch"] }
        "#,
    );
    p.root().join("Cargo.lock").rm_rf();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn patch_cargo_toml_raises_rust_version_for_preferred_patch() {
    Package::new("higher-msrv", "1.0.0")
        .rust_version("1.80.0")
        .file(
            "src/lib.rs",
            r#"pub fn version() -> &'static str { "higher-msrv 1.0.0" }"#,
        )
        .publish();
    Package::new("higher-msrv", "1.1.0")
        .rust_version("1.80.0")
        .file(
            "src/lib.rs",
            r#"pub fn version() -> &'static str { "higher-msrv 1.1.0" }"#,
        )
        .publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"
                rust-version = "1.85.0"
                resolver = "3"

                [dependencies]
                higher-msrv = "1"

                [patch.crates-io]
                higher-msrv = { version = "=1.1.0", patches = ["patches/higher-msrv.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() {}")
        .file(
            "patches/higher-msrv.patch",
            r#"
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -6 +6 @@
-        rust-version = "1.80.0"
+        rust-version = "1.999.0"
"#,
        )
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .run();
}

#[cargo_test]
fn patches_for_multiple_versions_of_same_package_are_isolated() {
    Package::new("foo", "0.1.0")
        .file(
            "src/lib.rs",
            "pub fn version() -> &'static str { \"0.1.0\" }\n",
        )
        .publish();
    Package::new("foo", "0.2.0")
        .file(
            "src/lib.rs",
            "pub fn version() -> &'static str { \"0.2.0\" }\n",
        )
        .publish();

    let p = project()
        .file(
            "Cargo.toml",
            r#"
                cargo-features = ["patch-files"]

                [package]
                name = "bar"
                edition = "2021"

                [dependencies]
                foo01 = { package = "foo", version = "0.1" }
                foo02 = { package = "foo", version = "0.2" }

                [patch.crates-io]
                foo01 = { package = "foo", version = "=0.1.0", patches = ["patches/foo-0.1.patch"] }
                foo02 = { package = "foo", version = "=0.2.0", patches = ["patches/foo-0.2.patch"] }
            "#,
        )
        .file(
            "src/main.rs",
            r#"
                fn main() {
                    println!("{} {}", foo01::version(), foo02::version());
                }
            "#,
        )
        .file(
            "patches/foo-0.1.patch",
            r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-pub fn version() -> &'static str { "0.1.0" }
+pub fn version() -> &'static str { "patched-0.1.0" }
"#,
        )
        .file(
            "patches/foo-0.2.patch",
            r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-pub fn version() -> &'static str { "0.2.0" }
+pub fn version() -> &'static str { "patched-0.2.0" }
"#,
        )
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
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
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .run();
}

#[cargo_test]
fn no_rebuild_if_no_patch_changed() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();

    p.cargo("run -v")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn rebuild_if_patch_changed() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
fn re_resolve_if_patch_removed_from_manifest() {
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
        .file("src/main.rs", "fn main() {}")
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("generate-lockfile")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
        .run();
}

#[cargo_test]
fn cargo_pkgid() {
    let p = patched_project();

    p.cargo("generate-lockfile")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
        .run();

    p.cargo("pkgid bar")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
        .run();
}

#[cargo_test]
fn cargo_metadata() {
    let p = patched_project();

    p.cargo("generate-lockfile")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
        .run();

    p.cargo("metadata")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .with_stdout_data(str![""])
        .with_status(101)
        .run();
}

#[cargo_test]
#[cfg(unix)]
fn patch_git_source_rejects_symlink_escape() {
    let outside = tempfile::NamedTempFile::new().unwrap();
    let git_project = git::new("bar", |project| {
        project
            .file("Cargo.toml", &basic_manifest("bar", "1.0.0"))
            .file("src/lib.rs", "// original\n")
            .symlink(outside.path(), "leak.txt")
    });
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

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

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
        .with_stdout_data(str![""])
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]]
            .unordered(),
        )
        .with_status(101)
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
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  unknown Cargo.toml feature `patch-files`

  See https://doc.rust-lang.org/nightly/cargo/reference/unstable.html for more information.

"#]])
        .run();
}
