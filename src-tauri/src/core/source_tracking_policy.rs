//! Source Tracking Policy evaluation (ADR-0018, spec §8.2).
//!
//! The default policy (`auto_release_tag_head`) deterministically selects:
//! the latest formal provider Release tag, else the highest stable SemVer
//! tag, else the latest normal tag reachable from the remote default
//! branch, else `HEAD`. Draft and prerelease releases only enter through an
//! explicit prerelease channel override; fixed tag/commit, branch and HEAD
//! overrides are always explicit. The module is pure: Git transport facts
//! (tags, reachability, default branch commit) are supplied by the caller.

use thiserror::Error;

use crate::core::git_source::is_commit_sha;

/// The closed Source Tracking Policy mode vocabulary of ADR-0018. It mirrors
/// the Catalog CHECK constraint; the preview and transition services own the
/// mode families and this module owns the deterministic selection.
pub const TRACKING_MODES: &[&str] = &[
    "auto_release_tag_head",
    "prerelease_channel",
    "fixed_tag",
    "fixed_commit",
    "branch",
    "head",
];

pub fn is_supported_tracking_mode(mode: &str) -> bool {
    TRACKING_MODES.contains(&mode)
}

pub use crate::seams::remote_provider::ProviderReleaseFact;
pub use crate::seams::source::GitTagFact;

pub const SELECTION_KIND_PROVIDER_RELEASE: &str = "provider_release";
pub const SELECTION_KIND_SEMVER_TAG: &str = "semver_tag";
pub const SELECTION_KIND_NORMAL_TAG: &str = "normal_tag";
pub const SELECTION_KIND_HEAD: &str = "head";
pub const SELECTION_KIND_PRERELEASE_CHANNEL: &str = "prerelease_channel";
pub const SELECTION_KIND_FIXED_TAG: &str = "fixed_tag";
pub const SELECTION_KIND_FIXED_COMMIT: &str = "fixed_commit";
pub const SELECTION_KIND_BRANCH: &str = "branch";

/// The deterministic outcome of one policy evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicySelection {
    /// One of the `SELECTION_KIND_*` constants; `git_source_releases
    /// .selection_kind` stores the same vocabulary.
    pub selection_kind: &'static str,
    /// The ref handed to the Git transport: a tag name, an exact commit
    /// SHA, a branch name or `HEAD`.
    pub selected_ref: String,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TrackingPolicyError {
    #[error("unsupported Source Tracking Policy mode '{0}'")]
    UnsupportedMode(String),
    #[error("the Source Tracking Policy mode '{0}' requires an explicit value")]
    MissingValue(String),
    #[error("the prerelease channel '{0}' has no matching preserved release")]
    NoMatchingRelease(String),
    #[error("the fixed commit '{0}' is not a full 40-hex commit SHA")]
    InvalidFixedCommit(String),
    #[error("the branch override '{0}' is not a safe Git ref")]
    UnsafeRef(String),
    #[error("the tag override '{0}' is not a safe Git ref")]
    UnsafeTag(String),
}

/// Evaluate one Source Tracking Policy against frozen provider/Git facts.
///
/// * `all_tags` — every tag of the fetched mirror (peeled to commit).
/// * `reachable_tags` — the subset of `all_tags` whose commit is an ancestor
///   of the remote default branch's commit (step 3 of the default policy).
pub fn evaluate(
    mode: &str,
    value: Option<&str>,
    releases: &[ProviderReleaseFact],
    all_tags: &[GitTagFact],
    reachable_tags: &[GitTagFact],
) -> Result<PolicySelection, TrackingPolicyError> {
    match mode {
        "auto_release_tag_head" => evaluate_default(releases, all_tags, reachable_tags),
        "prerelease_channel" => {
            let channel = required_value(mode, value)?;
            evaluate_prerelease_channel(channel, releases, all_tags)
        }
        "fixed_tag" => {
            let tag = required_value(mode, value)?;
            if !is_safe_ref(tag) {
                return Err(TrackingPolicyError::UnsafeTag(tag.into()));
            }
            Ok(PolicySelection {
                selection_kind: SELECTION_KIND_FIXED_TAG,
                selected_ref: tag.into(),
            })
        }
        "fixed_commit" => {
            let commit = required_value(mode, value)?;
            if !is_commit_sha(commit) {
                return Err(TrackingPolicyError::InvalidFixedCommit(commit.into()));
            }
            Ok(PolicySelection {
                selection_kind: SELECTION_KIND_FIXED_COMMIT,
                selected_ref: commit.into(),
            })
        }
        "branch" => {
            let branch = required_value(mode, value)?;
            if !is_safe_ref(branch) {
                return Err(TrackingPolicyError::UnsafeRef(branch.into()));
            }
            Ok(PolicySelection {
                selection_kind: SELECTION_KIND_BRANCH,
                selected_ref: branch.into(),
            })
        }
        "head" => Ok(PolicySelection {
            selection_kind: SELECTION_KIND_HEAD,
            selected_ref: "HEAD".into(),
        }),
        other => Err(TrackingPolicyError::UnsupportedMode(other.into())),
    }
}

fn required_value<'a>(mode: &str, value: Option<&'a str>) -> Result<&'a str, TrackingPolicyError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| TrackingPolicyError::MissingValue(mode.into()))
}

fn evaluate_default(
    releases: &[ProviderReleaseFact],
    all_tags: &[GitTagFact],
    reachable_tags: &[GitTagFact],
) -> Result<PolicySelection, TrackingPolicyError> {
    let formal = releases
        .iter()
        .filter(|release| !release.is_prerelease && !release.is_draft)
        .max_by(|left, right| latest_release_order(left, right))
        .map(|release| release.tag_name.clone());
    if let Some(tag) = formal {
        return Ok(PolicySelection {
            selection_kind: SELECTION_KIND_PROVIDER_RELEASE,
            selected_ref: tag.clone(),
        });
    }
    if let Some(fact) = highest_stable_semver_tag(all_tags) {
        return Ok(PolicySelection {
            selection_kind: SELECTION_KIND_SEMVER_TAG,
            selected_ref: fact.name.clone(),
        });
    }
    if let Some(fact) = latest_normal_tag(reachable_tags) {
        return Ok(PolicySelection {
            selection_kind: SELECTION_KIND_NORMAL_TAG,
            selected_ref: fact.name.clone(),
        });
    }
    Ok(PolicySelection {
        selection_kind: SELECTION_KIND_HEAD,
        selected_ref: "HEAD".into(),
    })
}

fn evaluate_prerelease_channel(
    channel: &str,
    releases: &[ProviderReleaseFact],
    tags: &[GitTagFact],
) -> Result<PolicySelection, TrackingPolicyError> {
    let channel_release = releases
        .iter()
        .filter(|release| {
            release.is_prerelease
                && !release.is_draft
                && prerelease_part(&release.tag_name)
                    .is_some_and(|part| channel_matches(channel, part))
        })
        .max_by(|left, right| latest_release_order(left, right))
        .map(|release| release.tag_name.clone());
    if let Some(tag) = channel_release {
        return Ok(PolicySelection {
            selection_kind: SELECTION_KIND_PRERELEASE_CHANNEL,
            selected_ref: tag.clone(),
        });
    }
    let channel_tag = tags
        .iter()
        .filter(|tag| {
            semver_parts(&tag.name)
                .and_then(|(_, _, _, prerelease)| prerelease)
                .is_some_and(|part| channel_matches(channel, part))
        })
        .max_by(|left, right| semver_tag_order(left, right))
        .map(|tag| tag.name.clone());
    match channel_tag {
        Some(tag) => Ok(PolicySelection {
            selection_kind: SELECTION_KIND_PRERELEASE_CHANNEL,
            selected_ref: tag,
        }),
        None => Err(TrackingPolicyError::NoMatchingRelease(channel.into())),
    }
}

fn latest_release_order(
    left: &ProviderReleaseFact,
    right: &ProviderReleaseFact,
) -> std::cmp::Ordering {
    // Newest publish wins; a missing time sorts as oldest; tag name breaks
    // ties deterministically.
    (left.published_at, left.tag_name.as_str()).cmp(&(right.published_at, right.tag_name.as_str()))
}

fn highest_stable_semver_tag(tags: &[GitTagFact]) -> Option<&GitTagFact> {
    tags.iter()
        .filter(|tag| {
            semver_parts(&tag.name).is_some_and(|(_, _, _, prerelease)| prerelease.is_none())
        })
        .max_by(|left, right| semver_tag_order(left, right))
}

fn latest_normal_tag(tags: &[GitTagFact]) -> Option<&GitTagFact> {
    tags.iter()
        .filter(|tag| !is_prerelease_tag(&tag.name))
        .max_by(|left, right| {
            // Newest creation time wins; ref name breaks ties ascending
            // (spec §8.2: "按创建时间及 ref name 确定排序").
            (
                left.created_epoch_secs,
                std::cmp::Reverse(left.name.as_str()),
            )
                .cmp(&(
                    right.created_epoch_secs,
                    std::cmp::Reverse(right.name.as_str()),
                ))
        })
}

fn semver_tag_order(left: &GitTagFact, right: &GitTagFact) -> std::cmp::Ordering {
    let left_parts = semver_parts(&left.name);
    let right_parts = semver_parts(&right.name);
    match (left_parts, right_parts) {
        (Some(l), Some(r)) => compare_semver(l, r),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => left.name.cmp(&right.name),
    }
}

fn is_prerelease_tag(name: &str) -> bool {
    semver_parts(name)
        .and_then(|(_, _, _, prerelease)| prerelease)
        .is_some()
}

fn channel_matches(channel: &str, prerelease_part: &str) -> bool {
    prerelease_part == channel || prerelease_part.starts_with(&format!("{channel}."))
}

/// `(major, minor, patch, prerelease)` when `name` is a SemVer tag (with an
/// optional `v`/`V` prefix). Prerelease is `None` for stable versions.
fn semver_parts(name: &str) -> Option<(u64, u64, u64, Option<&str>)> {
    let base = name.strip_prefix(|c| c == 'v' || c == 'V').unwrap_or(name);
    let (core, prerelease_and_build) = base.split_once('-').map_or((base, None), |(core, rest)| {
        (core, Some(rest.split('+').next().unwrap_or(rest)))
    });
    let core = core.split('+').next().unwrap_or(core);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, prerelease_and_build))
}

/// Prerelease identifier of a SemVer tag name, `None` when the name is not a
/// SemVer tag or the version is stable.
fn prerelease_part(name: &str) -> Option<&str> {
    semver_parts(name).and_then(|(_, _, _, prerelease)| prerelease)
}

fn compare_semver(
    left: (u64, u64, u64, Option<&str>),
    right: (u64, u64, u64, Option<&str>),
) -> std::cmp::Ordering {
    match (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)) {
        std::cmp::Ordering::Equal => match (left.3, right.3) {
            (Some(l), Some(r)) => prerelease_order(l, r),
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        },
        ordering => ordering,
    }
}

/// SemVer prerelease ordering: dot-separated identifiers, numeric identifiers
/// compare numerically, numeric identifiers sort before alphanumeric ones,
/// shorter sequences sort first when one is a prefix.
fn prerelease_order(left: &str, right: &str) -> std::cmp::Ordering {
    let left_ids = left.split('.');
    let right_ids = right.split('.').collect::<Vec<_>>();
    for (index, left_id) in left_ids.enumerate() {
        let Some(right_id) = right_ids.get(index) else {
            return std::cmp::Ordering::Greater;
        };
        let ordering = compare_prerelease_identifier(left_id, right_id);
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    if left.split('.').count() == right_ids.len() {
        std::cmp::Ordering::Equal
    } else {
        std::cmp::Ordering::Less
    }
}

fn compare_prerelease_identifier(left: &str, right: &str) -> std::cmp::Ordering {
    let left_numeric = left.chars().all(|c| c.is_ascii_digit()) && !left.is_empty();
    let right_numeric = right.chars().all(|c| c.is_ascii_digit()) && !right.is_empty();
    match (left_numeric, right_numeric) {
        (true, true) => sequence_length_aware_numeric_cmp(left, right),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => left.cmp(right),
    }
}

fn sequence_length_aware_numeric_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    match left.len().cmp(&right.len()) {
        std::cmp::Ordering::Equal => left.cmp(right),
        ordering => ordering,
    }
}

fn is_safe_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with("refs/")
        && !value.contains([' ', '~', '^', ':', '?', '*', '[', '\\', '\0', '>', '<', '|'])
        && !value.contains("..")
        && !value.contains("@{")
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.ends_with('.')
        && !value.ends_with(".lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, commit: &str, created: Option<i64>) -> GitTagFact {
        GitTagFact {
            name: name.into(),
            commit: commit.into(),
            created_epoch_secs: created,
        }
    }

    fn release(
        tag_name: &str,
        prerelease: bool,
        draft: bool,
        published: Option<u128>,
    ) -> ProviderReleaseFact {
        ProviderReleaseFact {
            tag_name: tag_name.into(),
            is_prerelease: prerelease,
            is_draft: draft,
            published_at: published,
        }
    }

    const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BRANCH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn default_policy_prefers_the_latest_formal_provider_release() {
        let selection = evaluate(
            "auto_release_tag_head",
            None,
            &[
                release("v1.0.0", false, false, Some(100)),
                release("v2.0.0", false, false, Some(200)),
                release("v3.0.0-beta.1", true, false, Some(300)),
                release("v3.0.0-rc.1", false, true, Some(300)),
            ],
            &[tag("v2.0.1", BRANCH, Some(7))],
            &[tag("v2.0.1", BRANCH, Some(7))],
        )
        .expect("formal release");
        assert_eq!(selection.selection_kind, SELECTION_KIND_PROVIDER_RELEASE);
        assert_eq!(selection.selected_ref, "v2.0.0");
    }

    #[test]
    fn default_policy_skips_prerelease_and_draft_releases() {
        let selection = evaluate(
            "auto_release_tag_head",
            None,
            &[
                release("v3.0.0-beta.1", true, false, Some(300)),
                release("v2.9.9", false, true, Some(290)),
            ],
            &[],
            &[],
        )
        .expect("no formal release falls through");
        assert_eq!(selection.selection_kind, SELECTION_KIND_HEAD);
        assert_eq!(selection.selected_ref, "HEAD");
    }

    #[test]
    fn default_policy_falls_back_to_the_highest_stable_semver_tag() {
        let selection = evaluate(
            "auto_release_tag_head",
            None,
            &[],
            &[
                tag("v2.0.0", MAIN, Some(1)),
                tag("v2.10.0", MAIN, Some(2)),
                tag("v2.9.0", MAIN, Some(3)),
                tag("v3.0.0-rc.1", MAIN, Some(4)),
                tag("not-semver", MAIN, Some(5)),
            ],
            &[],
        )
        .expect("stable semver");
        assert_eq!(selection.selection_kind, SELECTION_KIND_SEMVER_TAG);
        assert_eq!(selection.selected_ref, "v2.10.0");
    }

    #[test]
    fn default_policy_falls_back_to_the_latest_reachable_normal_tag() {
        let selection = evaluate(
            "auto_release_tag_head",
            None,
            &[],
            &[
                tag("unreachable", MAIN, Some(100)),
                tag("older", MAIN, Some(1)),
                tag("newer", MAIN, Some(2)),
                tag("v1.0.0-beta.1", MAIN, Some(3)),
                tag("same-name", MAIN, Some(2)),
            ],
            &[
                tag("older", MAIN, Some(1)),
                tag("newer", MAIN, Some(2)),
                tag("same-name", MAIN, Some(2)),
                tag("v1.0.0-beta.1", MAIN, Some(3)),
            ],
        )
        .expect("reachable ordinary tag");
        assert_eq!(selection.selection_kind, SELECTION_KIND_NORMAL_TAG);
        // unreachable excluded; prerelease excluded; newest creation wins;
        // equal creation ties by ref name ascending.
        assert_eq!(selection.selected_ref, "newer");
    }

    #[test]
    fn default_policy_tie_breaks_creation_time_then_ref_name() {
        let selection = evaluate(
            "auto_release_tag_head",
            None,
            &[],
            &[],
            &[
                tag("zeta", MAIN, Some(10)),
                tag("alpha", MAIN, Some(10)),
                tag("gamma", MAIN, Some(9)),
            ],
        )
        .expect("deterministic tie");
        assert_eq!(selection.selected_ref, "alpha");
    }

    #[test]
    fn default_policy_falls_back_to_head_when_no_tag_is_reachable() {
        let selection = evaluate(
            "auto_release_tag_head",
            None,
            &[],
            &[tag("unreachable", MAIN, Some(1))],
            &[],
        )
        .expect("HEAD fallback");
        assert_eq!(selection.selection_kind, SELECTION_KIND_HEAD);
        assert_eq!(selection.selected_ref, "HEAD");
    }

    #[test]
    fn prerelease_channel_selects_the_latest_matching_prerelease_release() {
        let selection = evaluate(
            "prerelease_channel",
            Some("beta"),
            &[
                release("v2.0.0-beta.1", true, false, Some(100)),
                release("v2.0.0-beta.2", true, false, Some(200)),
                release("v2.0.0-rc.1", true, false, Some(300)),
                release("v2.0.0", false, false, Some(400)),
            ],
            &[],
            &[],
        )
        .expect("beta channel release");
        assert_eq!(selection.selection_kind, SELECTION_KIND_PRERELEASE_CHANNEL);
        assert_eq!(selection.selected_ref, "v2.0.0-beta.2");
    }

    #[test]
    fn prerelease_channel_falls_back_to_matching_semver_prerelease_tags() {
        let selection = evaluate(
            "prerelease_channel",
            Some("rc"),
            &[],
            &[
                tag("v1.2.0-rc.1", MAIN, Some(1)),
                tag("v1.2.0-rc.2", MAIN, Some(2)),
            ],
            &[],
        )
        .expect("channel tag");
        assert_eq!(selection.selected_ref, "v1.2.0-rc.2");
    }

    #[test]
    fn prerelease_channel_without_a_match_fails_closed() {
        let error =
            evaluate("prerelease_channel", Some("nightly"), &[], &[], &[]).expect_err("closed");
        assert_eq!(
            error,
            TrackingPolicyError::NoMatchingRelease("nightly".into())
        );
    }

    #[test]
    fn explicit_overrides_select_the_requested_ref() {
        let fixed_tag = evaluate("fixed_tag", Some("v1.2.3"), &[], &[], &[]).expect("tag override");
        assert_eq!(fixed_tag.selection_kind, SELECTION_KIND_FIXED_TAG);
        assert_eq!(fixed_tag.selected_ref, "v1.2.3");

        let commit = "0123456789abcdef0123456789abcdef01234567";
        let fixed_commit =
            evaluate("fixed_commit", Some(commit), &[], &[], &[]).expect("commit override");
        assert_eq!(fixed_commit.selection_kind, SELECTION_KIND_FIXED_COMMIT);
        assert_eq!(fixed_commit.selected_ref, commit);

        let branch = evaluate("branch", Some("main"), &[], &[], &[]).expect("branch override");
        assert_eq!(branch.selection_kind, SELECTION_KIND_BRANCH);
        assert_eq!(branch.selected_ref, "main");

        let head = evaluate("head", None, &[], &[], &[]).expect("head override");
        assert_eq!(head.selection_kind, SELECTION_KIND_HEAD);
        assert_eq!(head.selected_ref, "HEAD");
    }

    #[test]
    fn overrides_reject_parameterised_modes_without_a_value() {
        for mode in ["prerelease_channel", "fixed_tag", "fixed_commit", "branch"] {
            assert_eq!(
                evaluate(mode, None, &[], &[], &[]).expect_err("missing value"),
                TrackingPolicyError::MissingValue(mode.into())
            );
        }
    }

    #[test]
    fn fixed_commit_requires_a_full_sha_and_refs_are_sanitized() {
        assert_eq!(
            evaluate("fixed_commit", Some("abc"), &[], &[], &[]).expect_err("short"),
            TrackingPolicyError::InvalidFixedCommit("abc".into())
        );
        assert!(matches!(
            evaluate("branch", Some("refs/heads/main"), &[], &[], &[]).expect_err("slashes"),
            TrackingPolicyError::UnsafeRef(_)
        ));
        assert!(matches!(
            evaluate("fixed_tag", Some("v1.0.0>tag"), &[], &[], &[]).expect_err("metachar"),
            TrackingPolicyError::UnsafeTag(_)
        ));
        assert!(matches!(
            evaluate("branch", Some("../main"), &[], &[], &[]).expect_err("dotdot"),
            TrackingPolicyError::UnsafeRef(_)
        ));
    }

    #[test]
    fn unsupported_modes_fail_closed() {
        assert_eq!(
            evaluate("fancy_auto", None, &[], &[], &[]).expect_err("unknown mode"),
            TrackingPolicyError::UnsupportedMode("fancy_auto".into())
        );
    }
}
