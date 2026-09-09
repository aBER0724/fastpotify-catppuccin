//! Daily update check against GitHub releases.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/aBER0724/fastpotify-catppuccin/releases/latest";

/// Update-check interval.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The version number, without a leading `v`.
    pub version: String,
    /// The release page, with every download.
    pub url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

/// The newest release, when it is newer than this build.
pub async fn newer_release(http: &reqwest::Client) -> Result<Option<Release>> {
    let latest: LatestRelease = http
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("unexpected release listing")?;
    let version = latest.tag_name.trim_start_matches('v').to_string();
    Ok(
        is_newer(&version, env!("CARGO_PKG_VERSION")).then_some(Release {
            version,
            url: latest.html_url,
        }),
    )
}

/// A Fastpotify base version and whether it is this fork's release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    numbers: [u64; 3],
    catppuccin: bool,
}

/// Parse `major.minor.patch` and this fork's stable
/// `major.minor.patch-catppuccin` release tags. Other suffixes are
/// prereleases and are deliberately ignored by update checks.
fn parse(version: &str) -> Option<Version> {
    let version = version.trim();
    let (numbers, catppuccin) = match version.split_once('-') {
        Some((numbers, "catppuccin")) => (numbers, true),
        Some(_) => return None,
        None => (version, false),
    };
    let mut parts = numbers.split('.').map(|part| part.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    if parts.next().is_some() {
        return None;
    }
    Some(Version {
        numbers,
        catppuccin,
    })
}

/// Whether `candidate` is a newer stable Catppuccin release than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.1.4", "0.1.3"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.1.2", "0.1.3"));
        assert!(!is_newer("0.2.0-rc1", "0.1.3"));
        assert!(!is_newer("nightly", "0.1.3"));
    }

    #[test]
    fn catppuccin_release_follows_its_base_version() {
        assert!(is_newer("0.7.1-catppuccin", "0.7.1"));
        assert!(is_newer("0.7.2-catppuccin", "0.7.1-catppuccin"));
        assert!(!is_newer("0.7.1-catppuccin", "0.7.1-catppuccin"));
        assert!(!is_newer("0.7.1-catppuccin", "0.7.2-catppuccin"));
        assert!(!is_newer("0.7.1-catppuccin.1", "0.7.1-catppuccin"));
        assert!(!is_newer("0.7.1-rc1", "0.7.0-catppuccin"));
    }
}
