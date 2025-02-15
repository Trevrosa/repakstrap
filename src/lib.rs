#![allow(clippy::missing_errors_doc)]

/// Contains deserializable github structs.
mod github;

use std::{path::Path, process::Command};

use anyhow::anyhow;
use github::{Release, ReleaseAsset};
use reqwest::{Client, Method, StatusCode};
use semver::Version;

pub const DOWNLOADS_NAME: &str = "dist";
pub const CHECKED_MARKER_NAME: &str = "CHECKED";
#[cfg(windows)]
pub const BINARY_NAME: &str = "repak.exe";
#[cfg(target_os = "linux")]
pub const BINARY_NAME: &str = "repak";
pub const APIKEY_ENV_VAR: &str = "REPAKSTRAP_APIKEY";

/// Get a formatted error chain of an [`anyhow::Error`] in reverse.
pub fn get_error_chain(err: &anyhow::Error) -> String {
    err.chain()
        .rev()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" => ")
}

trait PathExt {
    /// Get a [`Path`]'s parent or return an empty path.
    fn maybe_parent(&self) -> &Self;
}

impl PathExt for Path {
    fn maybe_parent(&self) -> &Self {
        self.parent().unwrap_or("".as_ref())
    }
}

/// Get a repak binary's version as a [`semver::Version`]
pub fn get_local_version(binary_path: &Path) -> anyhow::Result<Version> {
    let cli_version = Command::new(binary_path)
        .arg("--version")
        .output()
        .map_err(|err| {
            anyhow!(
                "tried to run `{:?} -V`",
                binary_path
                    .strip_prefix(binary_path.maybe_parent().maybe_parent())
                    .unwrap_or("no path".as_ref())
            )
            .context(err)
        })?
        .stdout;
    let cli_version = String::from_utf8_lossy(&cli_version);

    let cli_version = cli_version
        .trim()
        .split(' ')
        .last()
        .ok_or(anyhow!("could not parse local version."))?;

    Ok(Version::parse(cli_version)?)
}

/// Find the correct [`ReleaseAsset`] to download from an [`Iterator`] of [`ReleaseAsset`]s according to the platform.
pub fn find_download(assets: impl IntoIterator<Item = ReleaseAsset>) -> Option<ReleaseAsset> {
    #[cfg(windows)]
    const BINARY_END: &str = "windows-msvc.zip";
    #[cfg(target_os = "linux")]
    const BINARY_END: &str = "linux-gnu.tar.xz";

    assets.into_iter().find(|a| a.name.ends_with(BINARY_END))
}

/// Get the latest repak-rivals [`Release`].
pub async fn get_remote(client: &Client, api_key: Option<String>) -> anyhow::Result<Release> {
    const RELEASES_URL: &str =
        "https://api.github.com/repos/natimerry/repak-rivals/releases/latest";
    const USER_AGENT: &str =
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:136.0) Gecko/20100101 Firefox/136.0";
    let request = client
        .request(Method::GET, RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", USER_AGENT);

    let request = if let Some(api_key) = api_key {
        request.bearer_auth(api_key)
    } else {
        request
    }
    .build()?;

    let resp = client.execute(request).await?;
    match resp.status() {
        StatusCode::FORBIDDEN => Err(anyhow!(
            "got 403 on api request: {}",
            resp.text().await.map_or_else(
                |_| "no text could be parsed".to_string(),
                |t| t.trim().to_string()
            )
        )),
        StatusCode::OK => Ok(resp.json().await?),
        status => Err(anyhow!("unhandled status {status}")),
    }
}

/// Parse a [`Release`] to get it's [`semver::Version`]
pub fn get_remote_version(release: &Release) -> anyhow::Result<Version> {
    // prefer tag name over release name
    // use tag_name[1..] to skip the v (like in 'v0.0.1')
    let version =
        Version::parse(&release.tag_name[1..]).or_else(|_| Version::parse(&release.name))?;
    Ok(version)
}
