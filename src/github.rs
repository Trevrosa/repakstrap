use serde::Deserialize;

/// A release's asset. Does not contain all fields.
#[derive(Debug, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// A github release. Does not contain all fields.
///
/// See the github [docs](https://docs.github.com/en/rest/releases/releases?apiVersion=2022-11-28#get-the-latest-release) for more information
#[derive(Debug, Deserialize)]
pub struct Release {
    pub name: String,
    pub tag_name: String,
    pub assets: Vec<ReleaseAsset>,
}
