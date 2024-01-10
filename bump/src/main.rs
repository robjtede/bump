#![allow(dead_code)]

use std::{fmt::Write as _, fs};

use cargo_metadata::{Dependency, DependencyKind, Metadata, Package, PackageId};
use dialoguer::{theme::ColorfulTheme, FuzzySelect, Input, MultiSelect};
use itertools::Itertools as _;
use toml_edit::Document;

mod utils;

fn main() {
    let mut args = std::env::args().skip_while(|val| !val.starts_with("--manifest-path"));

    let mut cmd = cargo_metadata::MetadataCommand::new();

    match args.next() {
        Some(path) if path == "--manifest-path" => {
            cmd.manifest_path(args.next().unwrap());
        }
        Some(path) => {
            cmd.manifest_path(path.trim_start_matches("--manifest-path="));
        }
        None => {}
    };

    let metadata = cmd.exec().unwrap();

    let mut members = vec![];

    for member in PkgIter(&metadata, &metadata.workspace_members) {
        // dbg!(&metadata[pkg_id].manifest_path);
        // dbg!(&metadata[member].dependencies);

        let workspace_dependencies = member
            .dependencies
            .iter()
            .filter(|&dep| (dep.path.is_some() && dep.kind == DependencyKind::Normal))
            .map(|dep| {
                (
                    dep.name.clone(),
                    dep.req.clone().comparators.pop().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();

        // list of workspace members which have <member> in their dependencies
        let workspace_dependents = PkgIter(&metadata, &metadata.workspace_members)
            .filter_map(|pkg| {
                pkg.dependencies.iter().find_map(|dep| {
                    (dep.name == member.name).then_some((pkg.name.clone(), dep.clone()))
                })
            })
            .collect::<Vec<_>>();

        members.push((member.clone(), workspace_dependencies, workspace_dependents));
    }

    let prompts = members
        .iter()
        .map(|(member, dependencies, dependents)| {
            let Package { name, version, .. } = member;

            let has_changelog = member.read_changelog().is_some();

            member_prompt(name, version, dependencies, dependents, has_changelog)
        })
        .collect::<Vec<_>>();

    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("What package do you want to bump?")
        .items(&prompts)
        .default(0)
        .report(false)
        .interact()
        .unwrap();

    let (pkg, _dependencies, dependents) = &members[selection];

    println!("You chose: {}", pkg.name);

    if let Some(unreleased) = pkg.extract_unreleased() {
        println!("Changes since {}", pkg.version);
        println!("{unreleased}");
    };

    let new_version = Input::<String>::with_theme(&ColorfulTheme::default())
        .with_prompt("New version:")
        .validate_with(|input: &String| -> Result<(), String> {
            let Ok(v2) = semver::Version::parse(input) else {
                return Err(format!("{input} is not a valid SemVer string"));
            };

            if v2 <= pkg.version {
                return Err("New version must be higher than current version".to_owned());
            }

            Ok(())
        })
        .interact_text()
        .unwrap();

    let target_manifest = fs::read_to_string(&pkg.manifest_path).unwrap();
    let mut target_manifest = target_manifest.parse::<Document>().unwrap();

    utils::replace_toml_string(
        &mut target_manifest["package"]["version"],
        &new_version.to_string(),
    );

    fs::write(&pkg.manifest_path, target_manifest.to_string()).unwrap();

    match dependents.len() {
        0 => {}

        n => {
            println!(
                "There are {n} workspace members that depend on {}.",
                pkg.name
            );

            let update_items = dependents
                .iter()
                .map(|(dependent, dep)| (format!("{dependent} : {}", dep.req), true))
                .collect::<Vec<_>>();

            let selections = MultiSelect::with_theme(&ColorfulTheme::default())
                .with_prompt(
                    "Select the workspace members whose version requirement should be updated to 2:",
                )
                .items_checked(&update_items)
                .interact()
                .unwrap();

            for selection in selections {
                let (dependant_name, dep) = &dependents[selection];

                let cur_req = &dep.req;
                let new_req = match utils::updated_req(
                    cur_req,
                    &pkg.version,
                    &semver::Version::parse(&new_version).unwrap(),
                ) {
                    utils::SemverUpdateKind::CurrentRequirementDoesNotMatchVersion => {
                        eprintln!("CurrentRequirementDoesNotMatchVersion so not touching requirement on this crate");
                        continue;
                    }
                    utils::SemverUpdateKind::ExistingReqCompatible => {
                        eprintln!("ExistingReqCompatible so leaving it alone");
                        continue;
                    }
                    utils::SemverUpdateKind::UpdateReq(req) => req,
                };

                println!(
                    "in {} manifest, updating {} from {cur_req} => {new_req}",
                    dependant_name, pkg.name,
                );

                let dependent_manifest_path = members
                    .iter()
                    .find_map(|(pkg, _, _)| {
                        (&pkg.name == dependant_name).then_some(pkg.manifest_path.clone())
                    })
                    .unwrap();

                let dependent_manifest = fs::read_to_string(&dependent_manifest_path).unwrap();
                let mut dependent_manifest = dependent_manifest.parse::<Document>().unwrap();

                let manifest_pkg_key = dep.rename.as_deref().unwrap_or(&dep.name);

                utils::replace_toml_string(
                    &mut dependent_manifest["dependencies"][manifest_pkg_key]["version"],
                    utils::req_into_string(new_req),
                );

                fs::write(&dependent_manifest_path, dependent_manifest.to_string()).unwrap();
            }
        }
    };

    println!("Placing recommended commit message on clipboard");
    arboard::Clipboard::new()
        .unwrap()
        .set_text(format!(
            "chore({}): prepare release {new_version}",
            pkg.name,
        ))
        .unwrap();
}

/// Iterate over packages given their package IDs.
struct PkgIter<'a>(&'a Metadata, &'a [PackageId]);

impl<'a> Iterator for PkgIter<'a> {
    type Item = &'a Package;

    fn next(&mut self) -> Option<Self::Item> {
        let PkgIter(metadata, pkg_ids) = self;
        let pkg_id = pkg_ids.first()?;
        *pkg_ids = &pkg_ids[1..];
        Some(&metadata[pkg_id])
    }
}

fn member_prompt(
    name: &str,
    version: &semver::Version,
    dependencies: &[(String, String)],
    dependents: &[(String, Dependency)],
    has_changelog: bool,
) -> String {
    let mut prompt = String::new();

    write!(prompt, "{name} {version}").unwrap();

    let mut n_meta =
        !dependencies.is_empty() as u8 + !dependents.is_empty() as u8 + has_changelog as u8;

    let needs_brackets = n_meta > 0;

    if needs_brackets {
        write!(prompt, " (").unwrap();
    }

    if !dependencies.is_empty() {
        write!(prompt, "dependencies: {}", dependencies.len()).unwrap();
        n_meta = n_meta.saturating_sub(1);

        if n_meta > 0 {
            write!(prompt, ", ").unwrap();
        }
    }

    if !dependents.is_empty() {
        write!(prompt, "dependents: {}", dependents.len()).unwrap();
        n_meta = n_meta.saturating_sub(1);

        if n_meta > 0 {
            write!(prompt, ", ").unwrap();
        }
    }

    if has_changelog {
        write!(prompt, "changelog").unwrap();
        n_meta = n_meta.saturating_sub(1);

        if n_meta > 0 {
            write!(prompt, ", ").unwrap();
        }
    }

    if needs_brackets {
        write!(prompt, ")").unwrap();
    }

    prompt
}

trait Changelog {
    fn read_changelog(&self) -> Option<String>;

    fn extract_unreleased(&self) -> Option<String>;
}

impl Changelog for Package {
    fn read_changelog(&self) -> Option<String> {
        [
            self.manifest_path.with_file_name("CHANGELOG.md"),
            self.manifest_path.with_file_name("RELEASES.md"),
            self.manifest_path.with_file_name("CHANGES.md"),
        ]
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
    }

    fn extract_unreleased(&self) -> Option<String> {
        let changelog = self.read_changelog()?;

        let unreleased = changelog
            .lines()
            .skip_while(|line| !line.ends_with("Unreleased"))
            .skip(1)
            .take_while(|line| !line.ends_with(&self.version.to_string()))
            .join("\n");

        Some(unreleased)
    }
}
