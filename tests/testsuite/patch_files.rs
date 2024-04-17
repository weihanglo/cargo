//! Tests for unstable `patch-files` feature.

use cargo_test_support::basic_manifest;
use cargo_test_support::compare::assert_match_exact;
use cargo_test_support::git;
use cargo_test_support::paths;
use cargo_test_support::project;
use cargo_test_support::registry;
use cargo_test_support::registry::Package;
use cargo_test_support::Project;

const HELLO_PATCH: &'static str = r#"
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+pub fn hello() {
+    println!("Hello, patched!")
+}
"#;

const PATCHTOOL: &'static str = r#"
[patchtool]
path = ["patch", "-N", "-p1", "-i"]
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
                bar = { version = "=1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .file(".cargo/config.toml", PATCHTOOL)
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
                bar = { version = "=1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_status(101)
        .with_stderr(
            "\
[WARNING] ignoring `patches` on patch for `bar` in `[..]`; see [..] about the status of this feature.
[UPDATING] [..]
[ERROR] failed to resolve patches for `[..]`

Caused by:
  patch for `bar` in `[..]` points to the same source, but patches must point to different sources
",
        )
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
                bar = { version = "=1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .file(
            ".cargo/config.toml",
            r#"
                [patch.crates-io]
                bar = { version = "=1.0.0", patches = [] }
            "#,
        )
        .build();

    p.cargo("check")
        .with_status(101)
        .with_stderr(
            "\
[WARNING] ignoring `patches` on patch for `bar` in `[..]`; see [..] about the status of this feature.
[WARNING] [patch] in cargo config: ignoring `patches` on patch for `bar` in `[..]`; see [..] about the status of this feature.
[UPDATING] [..]
[ERROR] failed to resolve patches for `[..]`

Caused by:
  patch for `bar` in `[..]` points to the same source, but patches must point to different sources
",
        )
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
        .with_stderr(
            "\
[WARNING] unused manifest key: dependencies.bar.patches; see [..] about the status of this feature.
[UPDATING] `dummy-registry` index
[LOCKING] [..]
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[CHECKING] bar v1.0.0
[CHECKING] foo v0.0.0 ([CWD])
[FINISHED] `dev` profile [..]
",
        )
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
        .with_stderr(
            "\
[ERROR] failed to parse manifest at `[..]`

Caused by:
  patch for `bar` in `[..]` requires an exact version when patching with patch files
",
        )
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
                bar = { version = "=1.0.0", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr(
            "\
[ERROR] failed to parse manifest at `[..]`

Caused by:
  patch for `bar` in `[..]` requires at least one patch file when patching with patch files
",
        )
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
                bar = { version = "=1.0.0", registry = "alternative", patches = [] }
            "#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr(
            "\
[ERROR] failed to parse manifest at `[..]`

Caused by:
  patch for `bar` in `[..]` must refer to the same source when patching with patch files
",
        )
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
                bar = { path = "bar", patches = [""] }
            "#,
        )
        .file("src/lib.rs", "")
        .file("bar/Cargo.toml", &basic_manifest("bar", "1.0.0"))
        .file("bar/src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr(
            "\
[ERROR] failed to parse manifest at `[..]`

Caused by:
  patch for `bar` in `[..]` requires a registry source when patching with patch files
",
        )
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
        .with_stderr(
            "\
[ERROR] failed to parse manifest at `[..]`

Caused by:
  patch for `bar` in `[..]` requires a registry source when patching with patch files
",
        )
        .run();
}

#[cargo_test(requires_patch)]
fn patch() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[COMPILING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[COMPILING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!")
        .run();

    let actual = p.read_lockfile();
    let expected = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 3

[[package]]
name = "bar"
version = "1.0.0"
source = "patched+registry+https://github.com/rust-lang/crates.io-index?name=bar&version=1.0.0&patch=patches%2Fhello.patch"

[[package]]
name = "foo"
version = "0.0.0"
dependencies = [
 "bar",
]
"#;
    assert_match_exact(expected, &actual);
}

#[cargo_test(requires_patch)]
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
            &format!(
                r#"
                [patch.crates-io]
                bar = {{ version = "=1.0.0", patches = ["patches/hello.patch"] }}
                {PATCHTOOL}
            "#
            ),
        )
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("run -Zpatch-files")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[COMPILING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[COMPILING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!")
        .run();
}

#[cargo_test(requires_patch)]
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
                bar = { version = "=1.0.0", registry = "alternative", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .file(".cargo/config.toml", PATCHTOOL)
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `alternative` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `alternative`)
[PATCHING] bar v1.0.0 (registry `alternative`)
[LOCKING] [..]
[COMPILING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[COMPILING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!")
        .run();
}

#[cargo_test(requires_patch)]
fn patch_manifest_add_dep() {
    Package::new("bar", "1.0.0").publish();
    Package::new("baz", "1.0.0").publish();
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
                bar = { version = "=1.0.0", patches = ["patches/add-baz.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { }")
        .file(
            "patches/add-baz.patch",
            r#"
                --- a/Cargo.toml
                +++ b/Cargo.toml
                @@ -3,4 +3,5 @@
                             name = "bar"
                             version = "1.0.0"
                -            authors = []
                +            [dependencies]
                +            baz = "1"

                ---
            "#,
        )
        .file(".cargo/config.toml", PATCHTOOL)
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[DOWNLOADING] crates ...
[DOWNLOADED] baz v1.0.0 (registry `dummy-registry`)
[CHECKING] baz v1.0.0
[CHECKING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[CHECKING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
",
        )
        .run();
}

#[cargo_test(requires_patch)]
fn patch_package_version() {
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
                bar = { version = "=1.0.0", patches = ["patches/v2.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { }")
        .file(
            "patches/v2.patch",
            r#"
                --- a/Cargo.toml
                +++ b/Cargo.toml
                @@ -3,3 +3,3 @@
                             name = "bar"
                -            version = "1.0.0"
                +            version = "2.55.66"
                             authors = []

                --- a/src/lib.rs
                +++ b/src/lib.rs
                @@ -1,0 +1,1 @@
                +compile_error!("YOU SHALL NOT PASS!");
            "#,
        )
        .file(".cargo/config.toml", PATCHTOOL)
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr_contains(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[CHECKING] bar v2.55.66 (bar@1.0.0 with 1 patch file)
[ERROR] YOU SHALL NOT PASS!
",
        )
        .run();
}

#[cargo_test(requires_patch)]
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
                version = "=1.0.0"
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
        .file(".cargo/config.toml", PATCHTOOL)
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[COMPILING] bar v1.0.0 (bar@1.0.0 with 2 patch files)
[COMPILING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!\n¡Hola, patched!")
        .run();

    let actual = p.read_lockfile();
    let expected = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 3

[[package]]
name = "bar"
version = "1.0.0"
source = "patched+registry+https://github.com/rust-lang/crates.io-index?name=bar&version=1.0.0&patch=patches%2Fhello.patch&patch=..%2Fhola.patch"

[[package]]
name = "foo"
version = "0.0.0"
dependencies = [
 "bar",
]
"#;
    assert_match_exact(expected, &actual);
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
                bar = { version = "=1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[ERROR] failed to load source for dependency `bar`

Caused by:
  Unable to update bar@1.0.0 with 1 patch file

Caused by:
  failed to open file `patches/hello.patch`

Caused by:
  [..]
",
        )
        .run();
}

#[cargo_test]
fn patch_without_patchtool() {
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
                bar = { version = "=1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() { bar::hello(); }")
        .file("patches/hello.patch", HELLO_PATCH)
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_status(101)
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[ERROR] failed to load source for dependency `bar`

Caused by:
  Unable to update bar@1.0.0 with 1 patch file

Caused by:
  failed to apply patches

Caused by:
  missing `[patchtool]` for patching dependencies
",
        )
        .run();
}

#[cargo_test(requires_patch)]
fn no_rebuild_if_no_patch_changed() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[COMPILING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[COMPILING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!")
        .run();

    p.cargo("run -v")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[FRESH] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[FRESH] foo v0.0.0 ([CWD])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!")
        .run();
}

#[cargo_test(requires_patch)]
fn rebuild_if_patch_changed() {
    let p = patched_project();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[UPDATING] `dummy-registry` index
[DOWNLOADING] crates ...
[DOWNLOADED] bar v1.0.0 (registry `dummy-registry`)
[PATCHING] bar v1.0.0
[LOCKING] [..]
[COMPILING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[COMPILING] foo v0.0.0 ([..])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("Hello, patched!")
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

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr(
            "\
[PATCHING] bar v1.0.0
[COMPILING] bar v1.0.0 (bar@1.0.0 with 1 patch file)
[COMPILING] foo v0.0.0 ([CWD])
[FINISHED] `dev` profile [..]
[RUNNING] `target/debug/foo[EXE]`
",
        )
        .with_stdout("¡Hola, patched!")
        .run();
}

#[cargo_test(requires_patch)]
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
                bar = { version = "=1.0.0", patches = ["patches/hello.patch"] }
            "#,
        )
        .file("src/main.rs", "fn main() {}")
        .file("patches/hello.patch", HELLO_PATCH)
        .file(".cargo/config.toml", PATCHTOOL)
        .build();

    p.cargo("run")
        .masquerade_as_nightly_cargo(&["patch-files"])
        .with_stderr_contains(
            "\
[PATCHING] bar v1.0.0
[WARNING] Patch `bar v1.0.0 (bar@1.0.0 with 1 patch file)` was not used in the crate graph.
",
        )
        .run();

    let actual = p.read_lockfile();
    let expected = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 3

[[package]]
name = "bar"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "[..]"

[[package]]
name = "foo"
version = "0.0.0"
dependencies = [
 "bar",
]

[[patch.unused]]
name = "bar"
version = "1.0.0"
source = "patched+registry+https://github.com/rust-lang/crates.io-index?name=bar&version=1.0.0&patch=patches%2Fhello.patch"
"#;
    assert_match_exact(expected, &actual);
}
