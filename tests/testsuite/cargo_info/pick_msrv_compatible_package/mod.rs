use crate::prelude::*;
use cargo_test_support::file;
use cargo_test_support::registry::Package;

use super::init_registry_without_token;

#[cargo_test]
fn case() {
    init_registry_without_token();
    super::publish_packages(|batch| {
        Package::new("my-package", "0.1.1+my-package")
            .rust_version("1.0.0")
            .publish_to(batch);
        Package::new("my-package", "0.2.0+my-package")
            .rust_version("1.9876.0")
            .publish_to(batch);
    });

    snapbox::cmd::Command::cargo_ui()
        .arg("info")
        .arg("my-package")
        .arg("--registry=dummy-registry")
        .assert()
        .success()
        .stdout_eq(file!["stdout.term.svg"])
        .stderr_eq(file!["stderr.term.svg"]);
}
