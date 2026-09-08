//! Community releases advertise downloadable versions, never installable updates.
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
}
#[derive(Deserialize)]
pub struct CommunityRelease {
    pub tag_name: String,
    pub draft: bool,
    pub assets: Vec<ReleaseAsset>,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommunityUpdate {
    pub version: String,
    pub release_url: String,
}

fn version(value: &str) -> Option<[u64; 3]> {
    let parts: Vec<_> = value
        .strip_prefix('v')
        .unwrap_or(value)
        .split('.')
        .collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || (p.len() > 1 && p.starts_with('0'))
                || !p.bytes().all(|c| c.is_ascii_digit())
        })
    {
        return None;
    }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ])
}

/// Published prereleases use the same vX.Y.Z tag contract as stable releases.
pub fn select_update(current: &str, releases: &[CommunityRelease]) -> Option<CommunityUpdate> {
    let current = version(current)?;
    releases
        .iter()
        .filter_map(|release| {
            if !release.tag_name.starts_with('v') {
                return None;
            }
            let next = version(&release.tag_name)?;
            let asset = format!("Skill-Man-{}-aarch64.dmg", release.tag_name);
            if release.draft || next <= current || !release.assets.iter().any(|a| a.name == asset) {
                return None;
            }
            Some((next, release))
        })
        .max_by_key(|(next, _)| *next)
        .map(|(_, release)| CommunityUpdate {
            version: release.tag_name.trim_start_matches('v').to_owned(),
            release_url: format!(
                "https://github.com/RookieZoe/skill-man/releases/tag/{}",
                release.tag_name
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str, draft: bool) -> CommunityRelease {
        CommunityRelease {
            tag_name: tag.into(),
            draft,
            assets: vec![ReleaseAsset {
                name: format!("Skill-Man-{tag}-aarch64.dmg"),
            }],
        }
    }
    #[test]
    fn selects_numeric_newest_downloadable_release_and_ignores_drafts() {
        let releases = vec![
            release("v0.2.0", false),
            release("v0.10.0", false),
            release("v9.0.0", true),
            release("v99.0.0/evil", false),
        ];
        assert_eq!(select_update("0.1.0", &releases).unwrap().version, "0.10.0");
        assert!(select_update("0.10.0", &releases).is_none());
    }
    #[test]
    fn requires_a_downloadable_macos_asset() {
        let mut r = release("v0.2.0", false);
        r.assets.clear();
        assert!(select_update("0.1.0", &[r]).is_none());
    }
}
