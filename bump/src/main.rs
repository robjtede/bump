use std::{fmt::Write as _, fs, mem};

use cargo_metadata::{Dependency, DependencyKind, Metadata, Package, PackageId};
use dialoguer::{theme::ColorfulTheme, FuzzySelect, Input, MultiSelect};
use itertools::Itertools as _;
use toml_edit::Document;

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
            match semver::Version::parse(input) {
                Ok(_) => Ok(()),
                Err(_) => Err(format!("{input} is not a valid SemVer string")),
            }
        })
        .interact_text()
        .unwrap();

    let target_manifest = fs::read_to_string(&pkg.manifest_path).unwrap();
    let mut target_manifest = target_manifest.parse::<Document>().unwrap();

    let decor = mem::take(
        target_manifest["package"]["version"]
            .as_value_mut()
            .unwrap()
            .decor_mut(),
    );

    target_manifest["package"]["version"] = toml_edit::value(&new_version);
    *target_manifest["package"]["version"]
        .as_value_mut()
        .unwrap()
        .decor_mut() = decor;

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
                let (update_item, _) = &update_items[selection];

                println!(" updating {} => ^2", update_item);
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
