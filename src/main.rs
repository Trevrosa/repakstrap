use std::{
    env,
    fs::{self, File},
    io::{stdout, Write},
    path::Path,
    process::{exit, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use futures_util::StreamExt;
use humantime::format_duration;
use repakstrap::{
    find_download, get_error_chain, get_local_version, get_remote, get_remote_version,
    APIKEY_ENV_VAR, BINARY_NAME, CHECKED_MARKER_NAME, DOWNLOADS_NAME,
};
use reqwest::{self, Client};
use tokio::runtime;

fn unarchive(input: &Path, output: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    let unarchiver = Command::new("powershell")
        .args([
            "-Command",
            "Expand-Archive",
            &input.to_string_lossy(),
            "-DestinationPath",
            &output.to_string_lossy(),
            "-Force",
        ])
        .stdout(Stdio::null())
        .status();
    #[cfg(target_os = "linux")]
    let unarchiver = Command::new("tar")
        .args([
            "xf",
            &input.to_string_lossy(),
            "-C",
            &output.to_string_lossy(),
        ])
        .stdout(Stdio::null())
        .status();

    match unarchiver {
        Ok(status) if status.code() == Some(0) => {
            // the archived linux binaries are contained in a folder.
            #[cfg(target_os = "linux")]
            {
                const INNER_DIR: &str = "repak_cli-x86_64-unknown-linux-gnu";
                let linux_files = output.join(INNER_DIR);
                for file in linux_files.read_dir()? {
                    let file = file?;
                    if file.path().is_file() {
                        fs::rename(
                            file.path(),
                            file.path().to_string_lossy().replace(INNER_DIR, ""),
                        )?;
                    }
                }
                fs::remove_dir(linux_files)?;
            }

            Ok(())
        }
        Ok(status) => Err(anyhow!(
            "failed to extract {:?}, exited with code {status}",
            input.file_name()
        )),
        Err(err) => Err(anyhow!("trying to extract {:?}", input.file_name()).context(err)),
    }
}

async fn check_updates(
    client: &Client,
    download_path: &Path,
    binary_path: &Path,
    checked_marker_path: &Path,
) -> anyhow::Result<()> {
    let local_version = get_local_version(binary_path);

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
        writeln!(stdout, "\nstarting download")?;

        let downloaded = client.get(&download.browser_download_url).send().await?;

        let download_size = downloaded
            .content_length()
            .ok_or(anyhow!("could not get content_length"))?;

        let msg = format!("downloading {remote_version}/{}", download.name);

        let download_output = download_path.join(download.name);
        let mut file = File::create(&download_output)?;

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

        if let Err(err) = unarchive(&download_output, download_path) {
            println!("errors: {}\n", get_error_chain(&err));
        }
        writeln!(stdout, "extracted.\n")?;

        // drop stdout lock
        drop(stdout);

        Ok(())
    };

    match local_version {
        Ok(local_version) => {
            if remote_version > local_version {
                println!("found new version {remote_version}");
                download_and_unzip.await?;
                println!("summary: repak {local_version} => {remote_version}");
            } else {
                println!("you have the latest repak ({local_version})");
            }
        }
        Err(err) => {
            println!(
                "could not find local version!\nerrors: {}",
                get_error_chain(&err)
            );
            download_and_unzip.await?;
            println!("summary: got the latest repak {remote_version}");
        }
    }

    File::create(checked_marker_path)?;

    Ok(())
}

// 1 hour
const CHECK_COOLDOWN: Duration = Duration::from_secs(60 * 60);

async fn inner_main() -> anyhow::Result<()> {
    let current_dir = env::current_exe()?;
    let current_dir = current_dir
        .parent()
        .ok_or(anyhow!("could not get path of current executable."))?;

    let download_path = current_dir.join(DOWNLOADS_NAME);
    let binary_path = download_path.join(BINARY_NAME);
    let checked_marker_path = download_path.join(CHECKED_MARKER_NAME);

    let do_checks_and_download = async {
        if !download_path.exists() {
            fs::create_dir(&download_path)?;
        }

        // we are ok with update check failing
        if let Err(err) = check_updates(
            &Client::new(),
            &download_path,
            &binary_path,
            &checked_marker_path,
        )
        .await
        {
            println!(
                "failed to check for updates!\nerrors: {}",
                get_error_chain(&err)
            );
        }

        println!();

        Ok::<(), anyhow::Error>(())
    };

    let args = env::args().skip(1).collect::<Vec<_>>();
    // force update
    let args = if args.first().is_some_and(|a| a == "-U") {
        println!("forcing update checks.");
        do_checks_and_download.await?;
        // skip the -U
        &args[1..]
    } else {
        let checked_marker = fs::metadata(&checked_marker_path);
        // if we get any errors getting the modified time, default on doing the checks.
        let last_checked =
            checked_marker.map_or(None, |m| m.modified().map_or(None, |t| t.elapsed().ok()));

        // if `CHECKED_MARKER` exists and was modified less than `CHECK_COOLDOWN` ago, we can skip checks.
        match last_checked {
            Some(last_checked) if last_checked < CHECK_COOLDOWN => {
                println!(
                    "skipped checks, last checked `{}` ago.\n",
                    format_duration(last_checked)
                );
            }
            // not less than `CHECK_COOLDOWN`
            Some(_) => do_checks_and_download.await?,
            // `CHECKED_MARKER` doesn't exist or something went wrong getting modified time.
            None => do_checks_and_download.await?,
        }
        &args
    };

    if binary_path.exists() {
        let repak = Command::new(&binary_path)
            // the first arg is repakstrap
            .args(args)
            .status();
        match repak {
            Ok(repak) => exit(repak.code().unwrap_or(1)),
            Err(err) => println!("failed to run repak: {err}"),
        }
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
