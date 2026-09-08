//! Fixed public metadata endpoint. No credentials, package download or installation.
use crate::core::community_update::{CommunityRelease, CommunityUpdate, select_update};

pub async fn check(current_version: &str) -> Result<Option<CommunityUpdate>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Skill-Man-community-update-check")
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get("https://api.github.com/repos/RookieZoe/skill-man/releases?per_page=100")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err("Release metadata exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let releases: Vec<CommunityRelease> =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(select_update(current_version, &releases))
}
