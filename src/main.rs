use std::{
    env,
    fs::{self, File},
    io::Write,
    path::Path,
    process::{exit, Command},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use humantime::format_duration;
use repakstrap::{
    blocking_get_url, find_download, get_error_chain, get_local_version, get_remote,
    get_remote_version, APIKEY_ENV_VAR, BINARY_PATH, CHECKED_MARKER, DOWNLOAD_PATH,
};
use reqwest::blocking::{self, Client};

fn unzip(input: impl AsRef<Path>) -> anyhow::Result<()> {
    let input = input.as_ref();

    let unzip = Command::new("powershell")
        .args([
            "-Command",
            "Expand-Archive",
            &input.to_string_lossy(),
            "-DestinationPath",
            DOWNLOAD_PATH,
            "-Force",
        ])
        .status()?;

    if unzip.code() == Some(0) {
        Ok(())
    } else {
        Err(anyhow!("unzip failed"))
    }
}

fn check_updates(client: &blocking::Client) -> anyhow::Result<()> {
    let local_version = get_local_version();

    let remote = if let Ok(api_key) = env::var(APIKEY_ENV_VAR) {
        println!("using env api key.");
        get_remote(client, Some(api_key))
    } else {
        get_remote(client, None)
    }?;

    let remote_version = get_remote_version(&remote)?;

    let download_and_unzip = || {
        let Some(download) = find_download(remote.assets) else {
            return Err(anyhow!("could not find download url"));
        };
        println!("downloading {}..", &download.browser_download_url);

        let download_start = Instant::now();
        let downloaded = blocking_get_url(client, download.browser_download_url)?.bytes()?;
        println!("done! took {:?}", download_start.elapsed());

        let output = format!("{DOWNLOAD_PATH}/{}", download.name);

        let mut file = File::create(&output)?;
        file.write_all(&downloaded)?;
        drop(file);

        unzip(output)?;
        println!("unzipped.");

        Ok(())
    };

    match local_version {
        Ok(local_version) => {
            if remote_version > local_version {
                println!("found update {remote_version}");
                download_and_unzip()?;
            } else {
                println!("you have the latest repak ({local_version})");
            }
        }
        Err(err) => {
            println!(
                "could not find local version!\nerrors: {}\n",
                get_error_chain(&err)
            );
            download_and_unzip()?;
        }
    }

    fs::File::create(CHECKED_MARKER)?;

    Ok(())
}

// 1 hour
const CHECK_COOLDOWN: Duration = Duration::from_secs(60 * 60);

fn main() -> anyhow::Result<()> {
    assert!(std::env::consts::OS == "windows", "tool only for windows!");

    let do_checks_and_download = || -> anyhow::Result<()> {
        let download_path = Path::new(DOWNLOAD_PATH);
        if !download_path.exists() {
            fs::create_dir(download_path)?;
        }

        // we are ok with update check failing
        if let Err(err) = check_updates(&Client::new()) {
            println!(
                "failed to check for updates!\nerrors: {}\n",
                get_error_chain(&err)
            );
        }

        println!();

        Ok(())
    };

    let checked_marker = fs::metadata(CHECKED_MARKER);
    let last_checked =
        checked_marker.map_or(None, |m| m.modified().map_or(None, |t| t.elapsed().ok()));

    // if `CHECKED_MARKER` exists but was modified less than `CHECK_COOLDOWN` ago, skip checks.
    // if we get any errors getting the modified time, default on doing the checks.
    match last_checked {
        Some(last_checked) if last_checked >= CHECK_COOLDOWN => {
            do_checks_and_download()?;
        }
        Some(last_checked) if last_checked < CHECK_COOLDOWN => {
            println!(
                "skipped checks, last checked `{}` ago.\n",
                format_duration(last_checked)
            );
        }
        Some(_) => panic!("should not be reachable"),
        None => {
            do_checks_and_download()?;
        }
    }

    if Path::new(BINARY_PATH).exists() {
        let repak = Command::new(BINARY_PATH)
            .args(env::args().skip(1))
            .status()?;
        exit(repak.code().unwrap_or(1));
    } else {
        println!("repak binary not found.");
    }

    Ok(())
}
