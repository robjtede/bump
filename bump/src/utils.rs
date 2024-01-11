use std::{fmt, mem};

pub(crate) fn replace_toml_string_item(item: &mut toml_edit::Item, new_val: impl Into<String>) {
    replace_toml_string_value(item.as_value_mut().unwrap(), new_val)
}

pub(crate) fn replace_toml_string_value(item: &mut toml_edit::Value, new_val: impl Into<String>) {
    let decor = mem::take(item.decor_mut());
    *item = toml_edit::Value::String(toml_edit::Formatted::new(new_val.into()));
    *item.decor_mut() = decor;
}

#[derive(Debug, PartialEq)]
pub enum BumpKind {
    Patch,
    Minor,
    Major,
}

pub(crate) fn bump_kind(cur: &semver::Version, new: &semver::Version) -> BumpKind {
    assert!(new > cur, "new version must be higher than current version");

    if cur.major == 0 && cur.minor == 0 {
        // 0.0.x -> <anything> changes are always breaking
        return BumpKind::Major;
    }

    if cur.major == 0 {
        // pre-stable bumps

        if new.major > 0 {
            // stabilization bump (0.x -> 1.x)
            return BumpKind::Major;
        }

        if new.minor > cur.minor {
            // 0.x -> 0.y where y > x
            return BumpKind::Major;
        }

        // 0.x.y -> 0.x.z changes should always be treated as minor
        return BumpKind::Minor;
    }

    // stable version bumps

    if new.major > cur.major {
        // eg: 1.0.0 -> 2.0.0
        return BumpKind::Major;
    }

    if new.minor > cur.minor {
        // eg: 1.0.0 -> 1.2.0
        return BumpKind::Minor;
    }

    if new.patch > cur.patch {
        // eg: 1.0.0 -> 1.0.1
        return BumpKind::Patch;
    }

    unimplemented!("beta versions are not considered")
}

#[derive(Debug, PartialEq)]
pub(crate) enum SemverUpdateKind {
    /// Possibly an error or possible intended in the workspace if one crate is intentionally using
    /// an older version on the update target.
    CurrentRequirementDoesNotMatchVersion,

    /// Existing version requirement matches new version.
    ExistingReqCompatible,

    /// New requirement needed to match new version.
    UpdateReq(semver::VersionReq),
}

pub(crate) fn updated_req(
    req: &semver::VersionReq,
    v1: &semver::Version,
    v2: &semver::Version,
) -> SemverUpdateKind {
    if !req.matches(v1) {
        return SemverUpdateKind::CurrentRequirementDoesNotMatchVersion;
    }

    if req.matches(v2) {
        return SemverUpdateKind::ExistingReqCompatible;
    }

    match bump_kind(v1, v2) {
        BumpKind::Patch => SemverUpdateKind::ExistingReqCompatible,
        BumpKind::Minor => SemverUpdateKind::ExistingReqCompatible,
        BumpKind::Major => SemverUpdateKind::UpdateReq(to_min_req(v2)),
    }
}

pub(crate) fn to_min_req(ver: &semver::Version) -> semver::VersionReq {
    let ver = ver.to_string();
    semver::VersionReq::parse(ver.trim_end_matches(".0")).unwrap()
}

pub(crate) fn req_into_string(req: &semver::VersionReq) -> String {
    // forked from original Display implementations but adjusted so that caret
    // versions don't emit a ^ symbol

    #[derive(Debug, PartialEq)]
    struct VersionReqShort<'a> {
        comparators: Vec<ComparatorShort<'a>>,
    }

    impl fmt::Display for VersionReqShort<'_> {
        fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            if self.comparators.is_empty() {
                return formatter.write_str("*");
            }
            for (i, comparator) in self.comparators.iter().enumerate() {
                if i > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{}", comparator)?;
            }
            Ok(())
        }
    }

    #[derive(Debug, PartialEq)]
    struct ComparatorShort<'a>(&'a semver::Comparator);

    impl fmt::Display for ComparatorShort<'_> {
        fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            use semver::Op;

            let op = match self.0.op {
                Op::Exact => "=",
                Op::Greater => ">",
                Op::GreaterEq => ">=",
                Op::Less => "<",
                Op::LessEq => "<=",
                Op::Tilde => "~",
                Op::Caret => "",
                Op::Wildcard => "",
                _ => unimplemented!("other version req ops not supported"),
            };
            formatter.write_str(op)?;
            write!(formatter, "{}", self.0.major)?;
            if let Some(minor) = &self.0.minor {
                write!(formatter, ".{}", minor)?;
                if let Some(patch) = &self.0.patch {
                    write!(formatter, ".{}", patch)?;
                    if !self.0.pre.is_empty() {
                        write!(formatter, "-{}", self.0.pre)?;
                    }
                } else if self.0.op == Op::Wildcard {
                    formatter.write_str(".*")?;
                }
            } else if self.0.op == Op::Wildcard {
                formatter.write_str(".*")?;
            }
            Ok(())
        }
    }

    VersionReqShort {
        comparators: req.comparators.iter().map(ComparatorShort).collect(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! v {
        ($ver:literal) => {
            $ver.parse::<::semver::Version>().unwrap()
        };
    }

    macro_rules! req {
        ($req:literal) => {
            $req.parse::<::semver::VersionReq>().unwrap()
        };
    }

    #[test]
    fn version_to_min_req() {
        assert_eq!(to_min_req(&v!("0.0.1")), req!("0.0.1"));
        assert_eq!(to_min_req(&v!("0.1.0")), req!("0.1"));
        assert_eq!(to_min_req(&v!("0.1.1")), req!("0.1.1"));
        assert_eq!(to_min_req(&v!("1.0.0")), req!("1"));
        assert_eq!(to_min_req(&v!("1.1.0")), req!("1.1"));
        assert_eq!(to_min_req(&v!("1.0.1")), req!("1.0.1"));
    }

    #[test]
    fn analyze_bump_kind() {
        assert_eq!(bump_kind(&v!("0.0.1"), &v!("0.0.2")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("0.0.1"), &v!("0.1.0")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("0.1.0"), &v!("0.2.0")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("0.1.1"), &v!("0.2.0")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("0.1.1"), &v!("1.0.0")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("0.1.1"), &v!("1.1.0")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("1.0.0"), &v!("2.0.0")), BumpKind::Major);
        assert_eq!(bump_kind(&v!("1.0.5"), &v!("2.0.1")), BumpKind::Major);

        assert_eq!(bump_kind(&v!("0.1.0"), &v!("0.1.1")), BumpKind::Minor);
        assert_eq!(bump_kind(&v!("0.1.3"), &v!("0.1.7")), BumpKind::Minor);
        assert_eq!(bump_kind(&v!("1.0.0"), &v!("1.1.0")), BumpKind::Minor);
        assert_eq!(bump_kind(&v!("1.0.0"), &v!("1.2.3")), BumpKind::Minor);

        assert_eq!(bump_kind(&v!("1.0.0"), &v!("1.0.1")), BumpKind::Patch);
        assert_eq!(bump_kind(&v!("1.0.0"), &v!("1.0.3")), BumpKind::Patch);
        assert_eq!(bump_kind(&v!("1.2.3"), &v!("1.2.4")), BumpKind::Patch);
    }

    #[test]
    fn updated_semver_req() {
        assert_eq!(
            updated_req(&req!("1"), &v!("2.3.4"), &v!("2.3.5")),
            SemverUpdateKind::CurrentRequirementDoesNotMatchVersion,
        );

        assert_eq!(
            updated_req(&req!("1"), &v!("1.2.3"), &v!("1.2.4")),
            SemverUpdateKind::ExistingReqCompatible,
        );

        assert_eq!(
            updated_req(&req!("1.2"), &v!("1.2.3"), &v!("1.2.4")),
            SemverUpdateKind::ExistingReqCompatible,
        );

        assert_eq!(
            updated_req(&req!("1.2.3"), &v!("1.2.3"), &v!("1.2.4")),
            SemverUpdateKind::ExistingReqCompatible,
        );

        assert_eq!(
            updated_req(&req!("1"), &v!("1.3.4"), &v!("2.0.0")),
            SemverUpdateKind::UpdateReq(req!("2")),
        );
    }
}
