//! Tests for unstable `patch-files` feature.

use cargo_test_support::basic_manifest;
use cargo_test_support::git;
use cargo_test_support::paths;
use cargo_test_support::project;
use cargo_test_support::registry;
use cargo_test_support::registry::Package;

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
