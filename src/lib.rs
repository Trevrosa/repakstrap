use std::process::Command;

use anyhow::anyhow;
use const_format::concatcp;
use reqwest::{Client, Method, StatusCode};
use semver::Version;
use serde::Deserialize;

pub const DOWNLOAD_PATH: &str = "./dist/";
pub const CHECKED_MARKER: &str = concatcp!(DOWNLOAD_PATH, "CHECKED");
#[cfg(windows)]
pub const BINARY_PATH: &str = concatcp!(DOWNLOAD_PATH, "repak.exe");
#[cfg(target_os = "linux")]
pub const BINARY_PATH: &str = concatcp!(DOWNLOAD_PATH, "repak");
pub const APIKEY_ENV_VAR: &str = "REPAKSTRAP_APIKEY";

pub fn get_error_chain(err: &anyhow::Error) -> String {
    err.chain()
        .rev()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" => ")
}

pub fn get_local_version() -> anyhow::Result<Version> {
    let cli_version = Command::new(BINARY_PATH)
        .arg("--version")
        .output()
        .map_err(|err| anyhow!("tried to run `./dist/repak -V`").context(err))?
        .stdout;
    let cli_version = String::from_utf8_lossy(&cli_version);

    let cli_version = cli_version
        .trim()
        .split(' ')
        .last()
        .ok_or(anyhow!("could not parse local version."))?;

    Ok(Version::parse(cli_version)?)
}

#[derive(Debug, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// Does not contain all fields  
#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    pub name: String,
    pub tag_name: String,
    pub assets: Vec<GithubAsset>,
}

pub fn find_download(assets: impl IntoIterator<Item = GithubAsset>) -> Option<GithubAsset> {
    #[cfg(windows)]
    const BINARY_END: &str = "windows-msvc.zip";
    #[cfg(target_os = "linux")]
    const BINARY_END: &str = "linux-gnu.tar.xz";

    assets.into_iter().find(|a| a.name.ends_with(BINARY_END))
}

pub async fn get_remote(client: &Client, api_key: Option<String>) -> anyhow::Result<GithubRelease> {
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

pub fn get_remote_version(release: &GithubRelease) -> anyhow::Result<Version> {
    // prefer tag name over release name      skip v (in 'v0.0.1')
    let version =
        Version::parse(&release.tag_name[1..]).or_else(|_| Version::parse(&release.name))?;
    Ok(version)
}
