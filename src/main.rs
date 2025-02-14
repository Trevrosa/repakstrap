use std::{
    env,
    fs::{self, File},
    io::{stdout, Write},
    path::Path,
    process::{exit, Command},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use futures_util::StreamExt;
use humantime::format_duration;
use repakstrap::{
    find_download, get_error_chain, get_local_version, get_remote, get_remote_version,
    APIKEY_ENV_VAR, BINARY_PATH, CHECKED_MARKER, DOWNLOAD_PATH,
};
use reqwest::{self, Client};
use tokio::runtime;

fn unarchive(input: impl AsRef<Path>) -> anyhow::Result<()> {
    let input = input.as_ref();

    #[cfg(windows)]
    let unarchiver = Command::new("powershell")
        .args([
            "-Command",
            "Expand-Archive",
            &input.to_string_lossy(),
            "-DestinationPath",
            DOWNLOAD_PATH,
            "-Force",
        ])
        .status();
    #[cfg(target_os = "linux")]
    let unarchiver = Command::new("tar")
        .args(["xf", &input.to_string_lossy(), "-C", DOWNLOAD_PATH])
        .status();

    if unarchiver.is_ok_and(|s| s.code() == Some(0)) {
        #[cfg(target_os = "linux")]
        {
            let linux_files = Path::new(const_format::concatcp!(
                DOWNLOAD_PATH,
                "repak_cli-x86_64-unknown-linux-gnu"
            ));
            for file in linux_files.read_dir()? {
                let file = file?;
                if file.path().is_file() {
                    fs::rename(
                        file.path(),
                        file.path()
                            .to_string_lossy()
                            .replace("repak_cli-x86_64-unknown-linux-gnu", ""),
                    )?;
                }
            }
            fs::remove_dir(linux_files)?;
        }

        Ok(())
    } else {
        Err(anyhow!("unzip failed"))
    }
}

async fn check_updates(client: &Client) -> anyhow::Result<()> {
    let local_version = get_local_version();

    let remote = if let Ok(api_key) = env::var(APIKEY_ENV_VAR) {
        println!("using env api key.");
        get_remote(client, Some(api_key))
    } else {
        get_remote(client, None)
    }
    .await?;

    let remote_version = get_remote_version(&remote)?;

    let download_and_unzip = async {
        let Some(download) = find_download(remote.assets) else {
            return Err(anyhow!("could not find download url"));
        };

        let download_start = Instant::now();

        let mut stdout = stdout().lock();
        writeln!(stdout, "starting download")?;

        let downloaded = client.get(&download.browser_download_url).send().await?;

        let download_size = downloaded
            .content_length()
            .ok_or(anyhow!("could not get content_length"))?;

        let msg = format!("downloading {remote_version}/{}", download.name);

        let output = format!("{DOWNLOAD_PATH}/{}", download.name);
        let mut file = File::create(&output)?;

        let term_cols = termsize::get().map_or(0, |s| s.cols as usize);
        let mut progress = 0;

        let mut bytes_stream = downloaded.bytes_stream();
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            progress += chunk.len();

            let msg = format!("\r{msg}, {} bytes left", download_size - progress as u64);
            write!(stdout, "{msg}{}", " ".repeat(term_cols - msg.len()))?;
            stdout.flush()?;
        }

        // close file
        drop(file);

        writeln!(stdout, "\ndone! took {:?}", download_start.elapsed())?;

        if let Err(err) = unarchive(output) {
            println!("errors: {}\n", get_error_chain(&err));
        }
        writeln!(stdout, "extracted archive.")?;

        // drop stdout lock
        drop(stdout);

        Ok(())
    };

    match local_version {
        Ok(local_version) => {
            if remote_version > local_version {
                println!("found update {remote_version}");
                download_and_unzip.await?;
            } else {
                println!("you have the latest repak ({local_version})");
            }
        }
        Err(err) => {
            println!(
                "could not find local version!\nerrors: {}\n",
                get_error_chain(&err)
            );
            download_and_unzip.await?;
        }
    }

    fs::File::create(CHECKED_MARKER)?;

    Ok(())
}

// 1 hour
const CHECK_COOLDOWN: Duration = Duration::from_secs(60 * 60);

async fn inner_main() -> anyhow::Result<()> {
    let do_checks_and_download = async {
        let download_path = Path::new(DOWNLOAD_PATH);
        if !download_path.exists() {
            fs::create_dir(download_path)?;
        }

        // we are ok with update check failing
        if let Err(err) = check_updates(&Client::new()).await {
            println!(
                "failed to check for updates!\nerrors: {}\n",
                get_error_chain(&err)
            );
        }

        println!();

        Ok::<(), anyhow::Error>(())
    };

    let checked_marker = fs::metadata(CHECKED_MARKER);
    // if we get any errors getting the modified time, default on doing the checks.
    let last_checked =
        checked_marker.map_or(None, |m| m.modified().map_or(None, |t| t.elapsed().ok()));

    // if `CHECKED_MARKER` exists but was modified less than `CHECK_COOLDOWN` ago, skip checks.
    match last_checked {
        Some(last_checked) if last_checked < CHECK_COOLDOWN => {
            println!(
                "skipped checks, last checked `{}` ago.\n",
                format_duration(last_checked)
            );
        }
        // not less than `CHECK_COOLDOWN`
        Some(_) => do_checks_and_download.await?,
        None => do_checks_and_download.await?,
    }

    if Path::new(BINARY_PATH).exists() {
        let repak = Command::new(BINARY_PATH)
            // the first arg is repakstrap
            .args(env::args().skip(1))
            .status()?;
        exit(repak.code().unwrap_or(1));
    } else {
        println!("repak binary not found.");
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let rt = runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .worker_threads(1)
        .build()?;

    rt.block_on(inner_main())
}
