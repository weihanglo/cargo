//! Test if PGO works.

use cargo_test_support::prelude::*;
use cargo_test_support::project;
use cargo_test_support::str;

// macOS may emit different LLVM PGO warnings.
// Windows LLVM has different requirements.
#[cfg_attr(not(target_os = "linux"), cargo_test, ignore = "linux only")]
#[cfg_attr(target_os = "linux", cargo_test(requires = "llvm-profdata", nightly, reason = "don't run on rust-lang/rust CI"))]
fn pgo_works() {
    let p = project()
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "foo"
            edition = "2021"
            "#,
        )
        .file(
            "src/main.rs",
            r#"
                fn fibonacci(n: u64) -> u64 {
                    match n {
                        0 => 0,
                        1 => 1,
                        _ => fibonacci(n - 1) + fibonacci(n - 2),
                    }
                }

                fn main() {
                    for i in [15, 20, 25] {
                        let _ = fibonacci(i);
                    }
                }
            "#,
        )
        .build();

    let target_dir = p.build_dir();
    let release_bin = target_dir.join("release").join("foo");
    let pgo_data_dir = target_dir.join("pgo-data");
    let profdata_path = target_dir.join("merged.profdata");

    // Build the instrumented binary
    p.cargo("build --release")
        .env(
            "RUSTFLAGS",
            format!("-Cprofile-generate={}", pgo_data_dir.display()),
        )
        .run();
    // Run the instrumented binary
    cargo_test_support::execs()
        .with_process_builder(cargo_test_support::process(release_bin))
        .run();

    cargo_test_support::process("llvm-profdata")
        .arg("merge")
        .arg("-o")
        .arg(&profdata_path)
        .arg(pgo_data_dir)
        .status()
        .unwrap();

    // Use merged profdata during optimization.
    //
    // -Cllvm-args=-pgo-warn-missing-function is essential.
    // If there are LLVM warnings, there might be something wrong.
    p.cargo("build --release -v")
        .env(
            "RUSTFLAGS",
            format!(
                "-Cprofile-use={} -Cllvm-args=-pgo-warn-missing-function",
                profdata_path.display()
            ),
        )
        .with_stderr_data(str![[r#"
[DIRTY] foo v0.0.0 ([ROOT]/foo): the rustflags changed
[COMPILING] foo v0.0.0 ([ROOT]/foo)
[RUNNING] `rustc [..]-Cprofile-use=[ROOT]/foo/target/merged.profdata -Cllvm-args=-pgo-warn-missing-function`
[FINISHED] `release` profile [optimized] target(s) in [ELAPSED]s

"#]])
        .run();
}
