use std::collections::HashSet;
use std::fmt;
use std::fmt::Write;

use cargo::core::registry::PackageRegistry;
use cargo::core::QueryKind;
use cargo::core::Registry;
use cargo::core::SourceId;
use cargo::ops::Packages;
use cargo::util::command_prelude::*;

type Record = (String, Option<String>, String, bool);

pub fn cli() -> clap::Command {
    clap::Command::new("xtask-unpublished")
        .arg(flag(
            "check-version-bump",
            "check if any version bump is needed",
        ))
        .arg_package_spec_simple("Package to inspect the published status")
        .arg(
            opt(
                "verbose",
                "Use verbose output (-vv very verbose/build.rs output)",
            )
            .short('v')
            .action(ArgAction::Count)
            .global(true),
        )
        .arg_quiet()
        .arg(
            opt("color", "Coloring: auto, always, never")
                .value_name("WHEN")
                .global(true),
        )
        .arg(flag("frozen", "Require Cargo.lock and cache are up to date").global(true))
        .arg(flag("locked", "Require Cargo.lock is up to date").global(true))
        .arg(flag("offline", "Run without accessing the network").global(true))
        .arg(multi_opt("config", "KEY=VALUE", "Override a configuration value").global(true))
        .arg(
            Arg::new("unstable-features")
                .help("Unstable (nightly-only) flags to Cargo, see 'cargo -Z help' for details")
                .short('Z')
                .value_name("FLAG")
                .action(ArgAction::Append)
                .global(true),
        )
}

pub fn exec(args: &clap::ArgMatches, config: &mut cargo::util::Config) -> cargo::CliResult {
    config_configure(config, args)?;

    unpublished(args, config)?;

    Ok(())
}

fn config_configure(config: &mut Config, args: &ArgMatches) -> CliResult {
    let verbose = args.verbose();
    // quiet is unusual because it is redefined in some subcommands in order
    // to provide custom help text.
    let quiet = args.flag("quiet");
    let color = args.get_one::<String>("color").map(String::as_str);
    let frozen = args.flag("frozen");
    let locked = args.flag("locked");
    let offline = args.flag("offline");
    let mut unstable_flags = vec![];
    if let Some(values) = args.get_many::<String>("unstable-features") {
        unstable_flags.extend(values.cloned());
    }
    let mut config_args = vec![];
    if let Some(values) = args.get_many::<String>("config") {
        config_args.extend(values.cloned());
    }
    config.configure(
        verbose,
        quiet,
        color,
        frozen,
        locked,
        offline,
        &None,
        &unstable_flags,
        &config_args,
    )?;
    Ok(())
}

fn unpublished(args: &clap::ArgMatches, config: &mut cargo::util::Config) -> cargo::CliResult {
    let ws = args.workspace(config)?;

    let members_to_inspect: HashSet<_> = {
        let pkgs = args.packages_from_flags()?;
        if let Packages::Packages(_) = pkgs {
            HashSet::from_iter(pkgs.get_packages(&ws)?)
        } else {
            HashSet::from_iter(ws.members())
        }
    };

    let mut results = Vec::new();
    {
        let mut registry = PackageRegistry::new(config)?;
        let _lock = config.acquire_package_cache_lock()?;
        registry.lock_patches();
        let source_id = SourceId::crates_io(config)?;

        for member in members_to_inspect {
            let name = member.name();
            let current = member.version();
            if member.publish() == &Some(vec![]) {
                log::trace!("skipping {name}, `publish = false`");
                continue;
            }

            let version_req = format!("<={current}");
            let query =
                cargo::core::dependency::Dependency::parse(name, Some(&version_req), source_id)?;
            let possibilities = loop {
                // Exact to avoid returning all for path/git
                match registry.query_vec(&query, QueryKind::Exact) {
                    std::task::Poll::Ready(res) => {
                        break res?;
                    }
                    std::task::Poll::Pending => registry.block_until_ready()?,
                }
            };
            if let Some(last) = possibilities.iter().map(|s| s.version()).max() {
                let published = last == current;
                results.push((
                    name.to_string(),
                    Some(last.to_string()),
                    current.to_string(),
                    published,
                ));
            } else {
                results.push((name.to_string(), None, current.to_string(), false));
            }
        }
    }
    results.sort();

    if results.is_empty() {
        return Ok(());
    }

    let check_version_bump = args.flag("check-version-bump");

    if check_version_bump {
        output_version_bump_notice(&results);
    }

    output_table(results, check_version_bump)?;

    Ok(())
}

/// Outputs a markdown table of publish status for each members.
///
/// ```text
/// | name                            | crates.io | local  | published? |
/// | ----                            | --------- | -----  | ---------- |
/// | cargo                           | 0.70.1    | 0.72.0 | no         |
/// | cargo-credential                | 0.1.0     | 0.2.0  | no         |
/// | cargo-credential-1password      | 0.1.0     | 0.2.0  | no         |
/// | cargo-credential-gnome-secret   | 0.1.0     | 0.2.0  | no         |
/// | cargo-credential-macos-keychain | 0.1.0     | 0.2.0  | no         |
/// | cargo-credential-wincred        | 0.1.0     | 0.2.0  | no         |
/// | cargo-platform                  | 0.1.2     | 0.1.3  | no         |
/// | cargo-util                      | 0.2.3     | 0.2.4  | no         |
/// | crates-io                       | 0.36.0    | 0.36.1 | no         |
/// | home                            | 0.5.5     | 0.5.6  | no         |
/// ```
fn output_table(results: Vec<Record>, check_version_bump: bool) -> fmt::Result {
    let mut results: Vec<_> = results
        .into_iter()
        .filter(|(.., published)| !check_version_bump || *published)
        .map(|e| {
            (
                e.0,
                e.1.unwrap_or("-".to_owned()),
                e.2,
                if e.3 { "yes" } else { "no" }.to_owned(),
            )
        })
        .collect();

    if results.is_empty() {
        return Ok(());
    }

    let header = (
        "name".to_owned(),
        "crates.io".to_owned(),
        "local".to_owned(),
        if check_version_bump {
            "need version bump?"
        } else {
            "published?"
        }
        .to_owned(),
    );
    let separators = (
        "-".repeat(header.0.len()),
        "-".repeat(header.1.len()),
        "-".repeat(header.2.len()),
        "-".repeat(header.3.len()),
    );
    results.insert(0, header);
    results.insert(1, separators);

    let max_col_widths = results
        .iter()
        .map(|(name, last, local, bump)| (name.len(), last.len(), local.len(), bump.len()))
        .reduce(|(c0, c1, c2, c3), (f0, f1, f2, f3)| {
            (c0.max(f0), c1.max(f1), c2.max(f2), c3.max(f3))
        })
        .unwrap();

    let print_space = |out: &mut dyn Write, n| {
        for _ in 0..(n + 1) {
            write!(out, " ")?;
        }
        fmt::Result::Ok(())
    };

    let out = &mut String::new();
    for (name, last, local, bump) in results {
        write!(out, "| {name}")?;
        print_space(out, max_col_widths.0 - name.len())?;

        write!(out, "| {last}")?;
        print_space(out, max_col_widths.1 - last.len())?;

        write!(out, "| {local}")?;
        print_space(out, max_col_widths.2 - local.len())?;

        write!(out, "| {bump}")?;
        print_space(out, max_col_widths.3 - bump.len())?;

        writeln!(out, "|")?;
    }

    println!("{out}");

    Ok(())
}

fn output_version_bump_notice(results: &[Record]) {
    let pkgs_need_bump = results
        .iter()
        .filter_map(|(name, .., published)| published.then_some(name.clone()))
        .collect::<Vec<_>>();

    if !pkgs_need_bump.is_empty() {
        print!("### :warning: ");
        println!("Require at least a patch version bump for each of the following packages:\n");
        for pkg in pkgs_need_bump {
            println!("* {pkg}");
        }
        println!()
    }
}

#[test]
fn verify_cli() {
    cli().debug_assert();
}
