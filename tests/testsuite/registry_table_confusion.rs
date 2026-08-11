//! Scratch repros for reviewing rust-lang/cargo#17335:
//! how `[registry]` vs `[registries.<name>]` behave per field.
//! Not intended to land — companion to the review discussion at
//! <https://github.com/rust-lang/cargo/pull/17335#discussion_r3753041885>.

use crate::prelude::*;
use cargo_test_support::registry::{self, Package, RegistryBuilder, Token};
use cargo_test_support::{paths, project, str};

/// Mocked "now" for deterministic publish-age comparisons,
/// mirroring `min_publish_age.rs`.
const NOW: &str = "2006-08-08T00:00:00Z";

/// bar 1.0.0 is 14 days old, bar 1.1.0 is 2 days old relative to [`NOW`].
fn publish_aged_packages(alternative: bool) {
    Package::new("bar", "1.0.0")
        .alternative(alternative)
        .pubtime("2006-07-25T00:00:00Z")
        .publish();
    Package::new("bar", "1.1.0")
        .alternative(alternative)
        .pubtime("2006-08-06T00:00:00Z")
        .publish();
}

fn lockfile_project(registry: Option<&str>) -> cargo_test_support::Project {
    let dep = match registry {
        Some(name) => format!(r#"bar = {{ version = "1", registry = "{name}" }}"#),
        None => r#"bar = "1""#.to_owned(),
    };
    project()
        .file(
            "Cargo.toml",
            &format!(
                r#"
                [package]
                name = "foo"
                edition = "2021"

                [dependencies]
                {dep}
            "#
            ),
        )
        .file("src/lib.rs", "")
        .build()
}

fn publish_project() -> cargo_test_support::Project {
    project()
        .file(
            "Cargo.toml",
            r#"
                [package]
                name = "foo"
                version = "0.0.1"
                edition = "2015"
                authors = []
                license = "MIT"
                description = "foo"
            "#,
        )
        .file("src/main.rs", "fn main() {}")
        .build()
}

fn append_config(snippet: &str) {
    cargo_util::paths::append(&paths::cargo_home().join("config.toml"), snippet.as_bytes())
        .unwrap();
}

/// `registries.crates-io.token` is silently ignored for crates.io;
/// auth only ever reads the `[registry]` table for crates.io.
/// Contrast with `registries.crates-io.min-publish-age`, which is honored
/// and even overrides `registry.min-publish-age` (see `min_publish_age.rs`).
#[cargo_test]
fn registries_crates_io_token_ignored_for_crates_io() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registries.crates-io]\ntoken = \"{}\"\n",
        registry.token()
    ));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[ERROR] no token found, please run `cargo login`
or use environment variable CARGO_REGISTRY_TOKEN

"#]])
        .run();

    // The environment variable form is equally dead for crates.io.
    p.cargo("publish --no-verify")
        .env("CARGO_REGISTRIES_CRATES_IO_TOKEN", registry.token())
        .replace_crates_io(registry.index_url())
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[ERROR] no token found, please run `cargo login`
or use environment variable CARGO_REGISTRY_TOKEN

"#]])
        .run();
}

/// Control for the test above: the same token under `[registry]`
/// (still in config.toml, not credentials.toml) authenticates fine.
#[cargo_test]
fn registry_token_used_for_crates_io() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!("\n[registry]\ntoken = \"{}\"\n", registry.token()));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[WARNING] manifest has no documentation, homepage or repository
  |
  = [NOTE] see https://doc.rust-lang.org/cargo/reference/manifest.html#package-metadata for more info
[PACKAGING] foo v0.0.1 ([ROOT]/foo)
[PACKAGED] 4 files, [FILE_SIZE]B ([FILE_SIZE]B compressed)
[UPLOADING] foo v0.0.1 ([ROOT]/foo)
[UPLOADED] foo v0.0.1 to registry `crates-io`
[NOTE] waiting for foo v0.0.1 to be available at registry `crates-io`
[HELP] you may press ctrl-c to skip waiting; the crate should be available shortly
[PUBLISHED] foo v0.0.1 at registry `crates-io`

"#]])
        .run();
}

/// `registry.default` redirects which registry `cargo publish` targets,
/// but `registry.token` does NOT follow it: the token stays crates.io-only.
/// This is the existing behavior `registry.min-publish-age` mimics
/// (`registry_alt_ignores_min_publish_age` in `min_publish_age.rs`).
#[cargo_test]
fn registry_token_does_not_follow_registry_default() {
    let alt = RegistryBuilder::new()
        .alternative()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registry]\ndefault = \"alternative\"\ntoken = \"{}\"\n",
        alt.token()
    ));

    p.cargo("publish --no-verify")
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] `alternative` index
[ERROR] no token found for `alternative`, please run `cargo login --registry alternative`
or use environment variable CARGO_REGISTRIES_ALTERNATIVE_TOKEN

"#]])
        .run();
}

/// `registries.crates-io.credential-provider` is silently ignored, same as
/// the token: for crates.io only `registry.credential-provider` is consulted.
#[cargo_test]
fn registries_crates_io_credential_provider_ignored() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registries.crates-io]\ncredential-provider = [\"cargo:token-from-stdout\", \"echo\", \"{}\"]\n",
        registry.token()
    ));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[ERROR] no token found, please run `cargo login`
or use environment variable CARGO_REGISTRY_TOKEN

"#]])
        .run();
}

/// Control for the test above: the same provider under `[registry]` is used.
#[cargo_test]
fn registry_credential_provider_used_for_crates_io() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registry]\ncredential-provider = [\"cargo:token-from-stdout\", \"echo\", \"{}\"]\n",
        registry.token()
    ));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[WARNING] manifest has no documentation, homepage or repository
  |
  = [NOTE] see https://doc.rust-lang.org/cargo/reference/manifest.html#package-metadata for more info
[PACKAGING] foo v0.0.1 ([ROOT]/foo)
[PACKAGED] 4 files, [FILE_SIZE]B ([FILE_SIZE]B compressed)
[UPLOADING] foo v0.0.1 ([ROOT]/foo)
[UPLOADED] foo v0.0.1 to registry `crates-io`
[NOTE] waiting for foo v0.0.1 to be available at registry `crates-io`
[HELP] you may press ctrl-c to skip waiting; the crate should be available shortly
[PUBLISHED] foo v0.0.1 at registry `crates-io`

"#]])
        .run();
}

/// `registries.crates-io.secret-key` is silently ignored, same as `token`.
#[cargo_test]
fn registries_crates_io_secret_key_ignored() {
    let registry = RegistryBuilder::new()
        .http_api()
        .token(Token::rfc_key())
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registries.crates-io]\nsecret-key = \"{}\"\nsecret-key-subject = \"sub\"\n",
        registry.key()
    ));

    p.cargo("publish --no-verify -Zasymmetric-token")
        .masquerade_as_nightly_cargo(&["asymmetric-token"])
        .replace_crates_io(registry.index_url())
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[ERROR] no token found, please run `cargo login`
or use environment variable CARGO_REGISTRY_TOKEN

"#]])
        .run();
}

/// Control for the test above: the same key under `[registry]` works.
#[cargo_test]
fn registry_secret_key_used_for_crates_io() {
    let registry = RegistryBuilder::new()
        .http_api()
        .token(Token::rfc_key())
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registry]\nsecret-key = \"{}\"\nsecret-key-subject = \"sub\"\n",
        registry.key()
    ));

    p.cargo("publish --no-verify -Zasymmetric-token")
        .masquerade_as_nightly_cargo(&["asymmetric-token"])
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[WARNING] manifest has no documentation, homepage or repository
  |
  = [NOTE] see https://doc.rust-lang.org/cargo/reference/manifest.html#package-metadata for more info
[PACKAGING] foo v0.0.1 ([ROOT]/foo)
[PACKAGED] 4 files, [FILE_SIZE]B ([FILE_SIZE]B compressed)
[UPLOADING] foo v0.0.1 ([ROOT]/foo)
[UPLOADED] foo v0.0.1 to registry `crates-io`
[NOTE] waiting for foo v0.0.1 to be available at registry `crates-io`
[HELP] you may press ctrl-c to skip waiting; the crate should be available shortly
[PUBLISHED] foo v0.0.1 at registry `crates-io`

"#]])
        .run();
}

/// `global-credential-providers` only exists in `[registry]`;
/// under `[registries.crates-io]` it is silently ignored.
#[cargo_test]
fn registries_crates_io_global_credential_providers_ignored() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registries.crates-io]\nglobal-credential-providers = [\"cargo:token-from-stdout echo {}\"]\n",
        registry.token()
    ));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[ERROR] no token found, please run `cargo login`
or use environment variable CARGO_REGISTRY_TOKEN

"#]])
        .run();
}

/// ... and under `[registries.<alt>]` it is equally ignored.
/// It lives in the project config here because the harness already owns
/// the `[registries.alternative]` table in the home config.
#[cargo_test]
fn registries_alt_global_credential_providers_ignored() {
    let alt = RegistryBuilder::new()
        .alternative()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();
    p.change_file(
        ".cargo/config.toml",
        &format!(
            "[registries.alternative]\nglobal-credential-providers = [\"cargo:token-from-stdout echo {}\"]\n",
            alt.token()
        ),
    );

    p.cargo("publish --no-verify --registry alternative")
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] `alternative` index
[WARNING] unused config key `registries.alternative.global-credential-providers` in `[ROOT]/foo/.cargo/config.toml`
[ERROR] no token found for `alternative`, please run `cargo login --registry alternative`
or use environment variable CARGO_REGISTRIES_ALTERNATIVE_TOKEN

"#]])
        .run();
}

/// Control: `registry.global-credential-providers` works as a fallback
/// for crates.io (and any other registry).
#[cargo_test]
fn registry_global_credential_providers_used_for_crates_io() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registry]\nglobal-credential-providers = [\"cargo:token-from-stdout echo {}\"]\n",
        registry.token()
    ));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[WARNING] manifest has no documentation, homepage or repository
  |
  = [NOTE] see https://doc.rust-lang.org/cargo/reference/manifest.html#package-metadata for more info
[PACKAGING] foo v0.0.1 ([ROOT]/foo)
[PACKAGED] 4 files, [FILE_SIZE]B ([FILE_SIZE]B compressed)
[UPLOADING] foo v0.0.1 ([ROOT]/foo)
[UPLOADED] foo v0.0.1 to registry `crates-io`
[NOTE] waiting for foo v0.0.1 to be available at registry `crates-io`
[HELP] you may press ctrl-c to skip waiting; the crate should be available shortly
[PUBLISHED] foo v0.0.1 at registry `crates-io`

"#]])
        .run();
}

/// `registry.protocol` is not a key: even an invalid value is ignored,
/// though it does get an unused-key warning (from `GlobalRegistryConfig`
/// deserialization, which the min-publish-age code performs on resolve).
#[cargo_test]
fn registry_protocol_ignored() {
    Package::new("bar", "1.0.0").publish();
    let p = lockfile_project(None);

    append_config("\n[registry]\nprotocol = \"invalid\"\n");

    p.cargo("generate-lockfile")
        .with_stderr_data(str![[r#"
[WARNING] unused config key `registry.protocol` in `[ROOT]/home/.cargo/config.toml`
[UPDATING] `dummy-registry` index
[WARNING] unused config key `registry.protocol` in `[ROOT]/home/.cargo/config.toml`
[LOCKING] 1 package to highest compatible version

"#]])
        .run();
}

/// `registries.crates-io.protocol` IS read: an invalid value errors out.
#[cargo_test]
fn registries_crates_io_protocol_honored() {
    Package::new("bar", "1.0.0").publish();
    let p = lockfile_project(None);

    append_config("\n[registries.crates-io]\nprotocol = \"invalid\"\n");

    p.cargo("generate-lockfile")
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] unsupported registry protocol `invalid` (defined in [ROOT]/home/.cargo/config.toml)

"#]])
        .run();
}

/// `registries.<alt>.protocol` is parsed (no unused-key warning)
/// but never consulted: an invalid value passes silently.
#[cargo_test]
fn registries_alt_protocol_parsed_but_unused() {
    registry::alt_init();
    Package::new("bar", "1.0.0").alternative(true).publish();
    let p = lockfile_project(Some("alternative"));
    p.change_file(
        ".cargo/config.toml",
        "[registries.alternative]\nprotocol = \"invalid\"\n",
    );

    p.cargo("generate-lockfile")
        .with_stderr_data(str![[r#"
[UPDATING] `alternative` index
[LOCKING] 1 package to highest compatible version

"#]])
        .run();
}

/// `registry.index` is a hard error, kept from its pre-1.0 removal.
#[cargo_test]
fn registry_index_hard_error() {
    Package::new("bar", "1.0.0").publish();
    let p = lockfile_project(None);

    append_config("\n[registry]\nindex = \"https://example.com/\"\n");

    p.cargo("generate-lockfile")
        .with_status(101)
        .with_stderr_data(str![[r#"
[ERROR] failed to parse manifest at `[ROOT]/foo/Cargo.toml`

Caused by:
  the `registry.index` config value is no longer supported
  Use `[source]` replacement to alter the default index for crates.io.

"#]])
        .run();
}

/// `registries.crates-io.index` is silently ignored:
/// `SourceId::alt_registry` short-circuits the `crates-io` name
/// to the built-in index without reading it.
#[cargo_test]
fn registries_crates_io_index_ignored() {
    Package::new("bar", "1.0.0").publish();
    let p = lockfile_project(None);

    append_config("\n[registries.crates-io]\nindex = \"sparse+https://invalid.example.com/\"\n");

    p.cargo("generate-lockfile")
        .with_stderr_data(str![[r#"
[UPDATING] `dummy-registry` index
[LOCKING] 1 package to highest compatible version

"#]])
        .run();
}

/// `registries.crates-io.default` is ignored (with an unused-key warning);
/// only `registry.default` redirects the publish target.
#[cargo_test]
fn registries_crates_io_default_ignored() {
    let registry = RegistryBuilder::new()
        .http_api()
        .no_configure_token()
        .build();
    let p = publish_project();

    // If `default` were honored here, publish would fail:
    // no `alternative` registry is configured at all.
    append_config(&format!(
        "\n[registry]\ntoken = \"{}\"\n[registries.crates-io]\ndefault = \"alternative\"\n",
        registry.token()
    ));

    p.cargo("publish --no-verify")
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[WARNING] manifest has no documentation, homepage or repository
  |
  = [NOTE] see https://doc.rust-lang.org/cargo/reference/manifest.html#package-metadata for more info
[PACKAGING] foo v0.0.1 ([ROOT]/foo)
[WARNING] unused config key `registries.crates-io.default` in `[ROOT]/home/.cargo/config.toml`
[PACKAGED] 4 files, [FILE_SIZE]B ([FILE_SIZE]B compressed)
[UPLOADING] foo v0.0.1 ([ROOT]/foo)
[UPLOADED] foo v0.0.1 to registry `crates-io`
[NOTE] waiting for foo v0.0.1 to be available at registry `crates-io`
[HELP] you may press ctrl-c to skip waiting; the crate should be available shortly
[PUBLISHED] foo v0.0.1 at registry `crates-io`

"#]])
        .run();
}

/// `registries.crates-io.global-min-publish-age` is a no-op
/// (not a `RegistryConfig` field): it gets an unused-key warning
/// and the too-new version is still picked.
#[cargo_test]
fn registries_crates_io_global_min_publish_age_ignored() {
    publish_aged_packages(false);
    let p = lockfile_project(None);

    append_config("\n[registries.crates-io]\nglobal-min-publish-age = \"7 days\"\n");

    p.cargo("generate-lockfile")
        .env("__CARGO_TEST_INVOCATION_TIME", NOW)
        .with_stderr_data(str![[r#"
[WARNING] unused config key `registries.crates-io.global-min-publish-age` in `[ROOT]/home/.cargo/config.toml`
[UPDATING] `dummy-registry` index
[WARNING] unused config key `registries.crates-io.global-min-publish-age` in `[ROOT]/home/.cargo/config.toml`
[LOCKING] 1 package to highest compatible version

"#]])
        .run();
    assert!(p.read_lockfile().contains("1.1.0"));
}

/// ... and so is `registries.<alt>.global-min-publish-age`.
#[cargo_test]
fn registries_alt_global_min_publish_age_ignored() {
    registry::alt_init();
    publish_aged_packages(true);
    let p = lockfile_project(Some("alternative"));
    p.change_file(
        ".cargo/config.toml",
        "[registries.alternative]\nglobal-min-publish-age = \"7 days\"\n",
    );

    p.cargo("generate-lockfile")
        .env("__CARGO_TEST_INVOCATION_TIME", NOW)
        .with_stderr_data(str![[r#"
[WARNING] unused config key `registries.alternative.global-min-publish-age` in `[ROOT]/foo/.cargo/config.toml`
[UPDATING] `alternative` index
[WARNING] unused config key `registries.alternative.global-min-publish-age` in `[ROOT]/foo/.cargo/config.toml`
[LOCKING] 1 package to highest compatible version

"#]])
        .run();
    assert!(p.read_lockfile().contains("1.1.0"));
}

/// Auth resolves crates.io BY URL, before any name lookup:
/// a named registry whose index is crates.io's URL gets `[registry]`
/// credentials, and its own `registries.<name>.token` is ignored.
#[cargo_test]
fn mirror_of_crates_io_auth_resolves_by_url() {
    let registry = RegistryBuilder::new()
        .http_api()
        .http_index()
        .no_configure_token()
        .build();
    let p = publish_project();

    append_config(&format!(
        "\n[registries.mirror]\nindex = \"{}\"\ntoken = \"{}\"\n",
        registry.index_url(),
        registry.token()
    ));

    // The user said `--registry mirror`, but the URL matches crates.io,
    // so `registries.mirror.token` is ignored and the error is
    // crates.io-flavored.
    p.cargo("publish --no-verify --registry mirror")
        .replace_crates_io(registry.index_url())
        .with_status(101)
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[ERROR] no token found, please run `cargo login`
or use environment variable CARGO_REGISTRY_TOKEN

"#]])
        .run();

    // `[registry]` credentials unlock the "mirror" registry instead.
    append_config(&format!("\n[registry]\ntoken = \"{}\"\n", registry.token()));
    p.cargo("publish --no-verify --registry mirror")
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[PACKAGING] foo v0.0.1 ([ROOT]/foo)
[PACKAGED] 4 files, [FILE_SIZE]B ([FILE_SIZE]B compressed)
[UPLOADING] foo v0.0.1 ([ROOT]/foo)
[UPLOADED] foo v0.0.1 to registry `mirror`
[NOTE] waiting for foo v0.0.1 to be available at registry `mirror`
[HELP] you may press ctrl-c to skip waiting; the crate should be available shortly
[PUBLISHED] foo v0.0.1 at registry `mirror`

"#]])
        .run();
}

/// min-publish-age resolves BY NAME first: the same crates.io-URL mirror
/// honors `registries.mirror.min-publish-age` — the opposite lookup order
/// from auth above.
#[cargo_test]
fn mirror_of_crates_io_min_publish_age_resolves_by_name() {
    let registry = RegistryBuilder::new().no_configure_token().build();
    publish_aged_packages(false);
    let p = lockfile_project(Some("mirror"));

    append_config(&format!(
        "\n[registries.mirror]\nindex = \"{}\"\nmin-publish-age = \"7 days\"\n",
        registry.index_url()
    ));

    p.cargo("generate-lockfile")
        .env("__CARGO_TEST_INVOCATION_TIME", NOW)
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[LOCKING] 1 package to highest compatible version as of min-publish-age
[ADDING] bar v1.0.0 (available: v1.1.0, published 2 days ago)

"#]])
        .run();
    assert!(p.read_lockfile().contains("1.0.0"));
}

/// And when the mirror has no own min-publish-age, the "crates.io-only"
/// `registry.min-publish-age` leaks onto it through the URL match —
/// the very key that `registry_alt_ignores_min_publish_age` proves is
/// NOT applied to a registry with a non-crates.io URL.
#[cargo_test]
fn registry_min_publish_age_applies_to_crates_io_url_mirror() {
    let registry = RegistryBuilder::new().no_configure_token().build();
    publish_aged_packages(false);
    let p = lockfile_project(Some("mirror"));

    append_config(&format!(
        "\n[registries.mirror]\nindex = \"{}\"\n[registry]\nmin-publish-age = \"7 days\"\n",
        registry.index_url()
    ));

    p.cargo("generate-lockfile")
        .env("__CARGO_TEST_INVOCATION_TIME", NOW)
        .replace_crates_io(registry.index_url())
        .with_stderr_data(str![[r#"
[UPDATING] crates.io index
[LOCKING] 1 package to highest compatible version as of min-publish-age
[ADDING] bar v1.0.0 (available: v1.1.0, published 2 days ago)

"#]])
        .run();
    assert!(p.read_lockfile().contains("1.0.0"));
}
