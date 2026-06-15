//! Parse a `hatch plugin install <SOURCE>` argument into a concrete source.
//!
//! Resolution order (first match wins):
//! 1. An existing file on disk -> [`InstallSource::Local`].
//! 2. A GitHub reference: `github:owner/repo`, a `github.com/...` URL, or a
//!    bare `owner/repo` (optionally `@tag`) -> [`InstallSource::Github`].
//! 3. A bare registry plugin name (optionally `@version`) ->
//!    [`InstallSource::Registry`].

use anyhow::{bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSource {
    Local(PathBuf),
    Github { repo: String, tag: Option<String> },
    Registry { name: String, version: Option<String> },
}

static OWNER_REPO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$").unwrap());
static BARE_NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_-]*$").unwrap());

/// Split a trailing `@ref` (tag/version) off a reference, if present. Avoids
/// splitting on an `@` that is part of a path or URL by only honoring it when
/// the part before `@` still looks like a ref.
fn split_ref(s: &str) -> (&str, Option<&str>) {
    match s.rsplit_once('@') {
        Some((base, r)) if !base.is_empty() && !r.is_empty() => (base, Some(r)),
        _ => (s, None),
    }
}

pub fn parse(source: &str) -> Result<InstallSource> {
    let source = source.trim();
    if source.is_empty() {
        bail!("empty plugin source");
    }

    // 1. Existing local file always wins, before any pattern interpretation.
    let as_path = PathBuf::from(source);
    if as_path.is_file() {
        return Ok(InstallSource::Local(as_path));
    }

    // 2a. Explicit github: scheme or a github.com URL.
    if let Some(rest) = source.strip_prefix("github:") {
        let (base, tag) = split_ref(rest);
        let repo = normalize_repo(base)?;
        return Ok(InstallSource::Github { repo, tag: tag.map(str::to_string) });
    }
    if let Some(idx) = source.find("github.com") {
        // URL form. Only consider an `@tag` that trails the path part (after the
        // host) so the `git@github.com:` SSH host is never mistaken for a ref.
        let (host_part, path_part) = source.split_at(idx);
        let (base_path, tag) = split_ref(path_part);
        let repo = repo_from_url(&format!("{host_part}{base_path}"))?;
        let tag = tag.filter(|t| !t.contains('/'));
        return Ok(InstallSource::Github { repo, tag: tag.map(str::to_string) });
    }

    // 2b. Bare `owner/repo[@tag]`.
    let (base, refpart) = split_ref(source);
    if OWNER_REPO.is_match(base) {
        return Ok(InstallSource::Github {
            repo: base.to_string(),
            tag: refpart.map(str::to_string),
        });
    }

    // 3. Bare registry name `[@version]`.
    if BARE_NAME.is_match(base) {
        return Ok(InstallSource::Registry {
            name: base.to_string(),
            version: refpart.map(str::to_string),
        });
    }

    bail!(
        "could not interpret `{source}` as a local path, a GitHub repo (owner/repo), or a registry plugin name"
    );
}

/// Validate/normalize an `owner/repo` string.
fn normalize_repo(s: &str) -> Result<String> {
    let s = s.trim_end_matches(".git");
    if OWNER_REPO.is_match(s) {
        Ok(s.to_string())
    } else {
        bail!("`{s}` is not a valid GitHub `owner/repo`");
    }
}

/// Extract `owner/repo` from a GitHub URL (https or `git@` SSH form).
fn repo_from_url(url: &str) -> Result<String> {
    // Strip scheme and any `git@github.com:` / `github.com/` prefix.
    let after = url
        .split("github.com")
        .nth(1)
        .unwrap_or("")
        .trim_start_matches([':', '/']);
    let mut it = after.split('/');
    let owner = it.next().unwrap_or("");
    let repo = it.next().unwrap_or("");
    let repo = repo.split(['#', '?']).next().unwrap_or(repo);
    let repo = repo.trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() {
        bail!("could not parse owner/repo from GitHub URL `{url}`");
    }
    Ok(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo() {
        assert_eq!(
            parse("mario-chamuty/hatch-ios-plugin").unwrap(),
            InstallSource::Github { repo: "mario-chamuty/hatch-ios-plugin".into(), tag: None }
        );
    }

    #[test]
    fn parses_owner_repo_with_tag() {
        assert_eq!(
            parse("mario-chamuty/hatch-ios-plugin@v0.1.0").unwrap(),
            InstallSource::Github {
                repo: "mario-chamuty/hatch-ios-plugin".into(),
                tag: Some("v0.1.0".into()),
            }
        );
    }

    #[test]
    fn parses_github_scheme() {
        assert_eq!(
            parse("github:owner/repo").unwrap(),
            InstallSource::Github { repo: "owner/repo".into(), tag: None }
        );
    }

    #[test]
    fn parses_https_url() {
        assert_eq!(
            parse("https://github.com/owner/repo").unwrap(),
            InstallSource::Github { repo: "owner/repo".into(), tag: None }
        );
        assert_eq!(
            parse("https://github.com/owner/repo.git").unwrap(),
            InstallSource::Github { repo: "owner/repo".into(), tag: None }
        );
    }

    #[test]
    fn parses_bare_registry_name() {
        assert_eq!(
            parse("ios").unwrap(),
            InstallSource::Registry { name: "ios".into(), version: None }
        );
        assert_eq!(
            parse("ios@1.2.3").unwrap(),
            InstallSource::Registry { name: "ios".into(), version: Some("1.2.3".into()) }
        );
    }
}
