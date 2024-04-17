//! Tests for unstable `patch-files` feature.

use cargo_test_support::registry::Package;
use cargo_test_support::project;

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
