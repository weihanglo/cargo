use crate::prelude::*;
use cargo_test_support::file;
use cargo_test_support::registry::Package;

use super::init_registry_without_token;

#[cargo_test]
fn case() {
    init_registry_without_token();
    super::publish_packages(|batch| {
        for ver in ["0.1.1+my-package", "0.2.0+my-package", "0.2.3+my-package"] {
            Package::new("my-package", ver).publish_to(batch);
        }
    });
    snapbox::cmd::Command::cargo_ui()
        .arg("info")
        .arg("my-package@0.2")
        .arg("--registry=dummy-registry")
        .assert()
        .success()
        .stdout_eq(file!["stdout.term.svg"])
        .stderr_eq(file!["stderr.term.svg"]);
}
