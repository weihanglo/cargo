use crate::prelude::*;
use cargo_test_support::project;
use cargo_test_support::str;

/// Test that invalid SPDX license expressions with slash operator are handled
#[cargo_test]
fn invalid_license_expression_slash_operator() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT / Apache-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test that invalid SPDX license expressions with lowercase operators are handled
#[cargo_test]
fn invalid_license_expression_lowercase_operators() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT and Apache-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test that malformed license expressions are handled
#[cargo_test]
fn malformed_license_expression() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT OR (Apache-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test that valid SPDX license expressions are handled correctly
#[cargo_test]
fn valid_license_expression() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test that complex valid SPDX expressions are handled correctly
#[cargo_test]
fn complex_valid_license_expression() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "GPL-3.0-or-later WITH Classpath-exception-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test that packages without license field are handled correctly
#[cargo_test]
fn no_license_field() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test lint configuration scenarios
#[cargo_test]
fn lint_configuration_deny() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT / Apache-2.0"

[lints.cargo]
invalid_license_expression = "deny"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    // Test lint configuration with deny level
    p.cargo("check -Zcargo-lints")
        .masquerade_as_nightly_cargo(&["cargo-lints"])
        .with_stderr_data(str![[r#"
[WARNING] unknown lint: `invalid_license_expression`
 --> Cargo.toml:9:1
  |
9 | invalid_license_expression = "deny"
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = [NOTE] `cargo::unknown_lints` is set to `warn` by default
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test workspace-level lint configuration
#[cargo_test]
fn workspace_lint_configuration() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[workspace]
members = ["foo"]
resolver = "2"

[workspace.lints.cargo]
invalid_license_expression = "warn"
"#,
        )
        .file(
            "foo/Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT / Apache-2.0"

[lints]
workspace = true
"#,
        )
        .file("foo/src/lib.rs", "")
        .build();

    // Test workspace-level lint configuration
    p.cargo("check -Zcargo-lints")
        .masquerade_as_nightly_cargo(&["cargo-lints"])
        .with_stderr_data(str![[r#"
[WARNING] unknown lint: `invalid_license_expression`
 --> Cargo.toml:7:1
  |
7 | invalid_license_expression = "warn"
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
[NOTE] `cargo::invalid_license_expression` was inherited
 --> foo/Cargo.toml:9:1
  |
9 | workspace = true
  | ----------------
  |
  = [NOTE] `cargo::unknown_lints` is set to `warn` by default
[CHECKING] foo v0.1.0 ([ROOT]/foo/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test that lint configuration with "allow" level works correctly
#[cargo_test]
fn lint_configuration_allow() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"
license = "MIT / Apache-2.0"

[lints.cargo]
invalid_license_expression = "allow"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    // Test lint configuration with allow level
    p.cargo("check -Zcargo-lints")
        .masquerade_as_nightly_cargo(&["cargo-lints"])
        .with_stderr_data(str![[r#"
[WARNING] unknown lint: `invalid_license_expression`
 --> Cargo.toml:9:1
  |
9 | invalid_license_expression = "allow"
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = [NOTE] `cargo::unknown_lints` is set to `warn` by default
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test different Cargo editions with invalid license expressions
#[cargo_test]
fn edition_2024_invalid_license() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2024"
license = "MIT / Apache-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}

/// Test future edition behavior with invalid license expressions
#[cargo_test(nightly, reason = "future edition is always unstable")]
fn edition_future_invalid_license() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
cargo-features = ["unstable-editions"]

[package]
name = "foo"
version = "0.1.0"
edition = "future"
license = "MIT / Apache-2.0"
"#,
        )
        .file("src/lib.rs", "")
        .build();

    p.cargo("check")
        .masquerade_as_nightly_cargo(&["unstable-editions"])
        .with_stderr_data(str![[r#"
[CHECKING] foo v0.1.0 ([ROOT]/foo)
[FINISHED] `dev` profile [unoptimized + debuginfo] target(s) in [ELAPSED]s

"#]])
        .run();
}