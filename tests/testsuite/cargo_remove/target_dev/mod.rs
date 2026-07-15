use crate::prelude::*;
use cargo_test_support::Project;
use cargo_test_support::compare::assert_ui;
use cargo_test_support::current_dir;
use cargo_test_support::file;
use cargo_test_support::registry::Package;
use cargo_test_support::str;

#[cargo_test]
fn case() {
    cargo_test_support::registry::init();
    super::publish_packages(|packages| {
        Package::new("clippy", "0.4.0+my-package").publish_to(packages);
        Package::new("dbus", "0.6.2+my-package").publish_to(packages);
        Package::new("docopt", "0.6.2+my-package").publish_to(packages);
        Package::new("ncurses", "20.0.0+my-package").publish_to(packages);
        Package::new("regex", "0.1.1+my-package").publish_to(packages);
        Package::new("rustc-serialize", "0.4.0+my-package").publish_to(packages);
        Package::new("toml", "0.1.1+my-package").publish_to(packages);
        Package::new("semver", "0.1.1")
            .feature("std", &[])
            .publish_to(packages);
        Package::new("serde", "1.0.90")
            .feature("std", &[])
            .publish_to(packages);
    });

    let project = Project::from_template(current_dir!().join("in"));
    let project_root = project.root();
    let cwd = &project_root;

    snapbox::cmd::Command::cargo_ui()
        .arg("remove")
        .args(["--dev", "--target", "wasm32-unknown-unknown", "ncurses"])
        .current_dir(cwd)
        .assert()
        .success()
        .stdout_eq(str![""])
        .stderr_eq(file!["stderr.term.svg"]);

    assert_ui().subset_matches(current_dir!().join("out"), &project_root);
}
