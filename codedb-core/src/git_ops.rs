use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};

/// Derive a local path for a repo from its URL.
/// "https://github.com/ylow/SFrameRust/" -> "github.com/ylow/SFrameRust.git"
pub fn repo_dir_from_url(url: &str) -> Result<String> {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("git://"))
        .unwrap_or(url);

    let cleaned = stripped.trim_end_matches('/').trim_end_matches(".git");
    if cleaned.is_empty() {
        anyhow::bail!("Invalid repo URL: {url}");
    }
    Ok(format!("{cleaned}.git"))
}

/// Clone a bare repo, or fetch if it already exists.
/// Returns the opened gix::Repository.
pub fn clone_or_fetch(url: &str, repo_path: &Path) -> Result<gix::Repository> {
    if repo_path.exists() {
        let repo = gix::open(repo_path).context("Failed to open existing repo")?;
        fetch(&repo)?;
        Ok(repo)
    } else {
        clone_bare(url, repo_path)
    }
}

fn clone_bare(url: &str, path: &Path) -> Result<gix::Repository> {
    std::fs::create_dir_all(path)?;
    let mut prepare =
        gix::prepare_clone_bare(url, path).context("Failed to prepare clone")?;

    let (repo, _outcome) = prepare
        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
        .context("Failed to fetch during clone")?;

    Ok(repo)
}

fn fetch(repo: &gix::Repository) -> Result<()> {
    let remote = repo
        .find_remote("origin")
        .context("Failed to find remote 'origin'")?;

    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .context("Failed to connect to remote")?;

    connection
        .prepare_fetch(gix::progress::Discard, Default::default())
        .context("Failed to prepare fetch")?
        .receive(gix::progress::Discard, &AtomicBool::new(false))
        .context("Failed to receive fetch data")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_dir_from_url() {
        assert_eq!(
            repo_dir_from_url("https://github.com/ylow/SFrameRust/").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
        assert_eq!(
            repo_dir_from_url("https://github.com/ylow/SFrameRust.git").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
        assert_eq!(
            repo_dir_from_url("https://github.com/ylow/SFrameRust").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
    }

    #[test]
    fn test_repo_dir_from_url_git_protocol() {
        assert_eq!(
            repo_dir_from_url("git://github.com/ylow/SFrameRust").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
    }

    #[test]
    fn test_repo_dir_from_url_invalid() {
        assert!(repo_dir_from_url("https://").is_err());
    }
}
