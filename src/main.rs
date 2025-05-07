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
    find_downloads, get_error_chain, get_local_version, get_remote, get_remote_version,
    APIKEY_ENV_VAR, BINARY_NAME, CHECKED_MARKER_NAME, DOWNLOADS_NAME,
};
use reqwest::{self, Client};
use tokio::runtime;

fn extract_archive(input: &Path, output: &Path) -> anyhow::Result<()> {
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
                let inner_dir = input
                    .file_name()
                    .expect("file has no name?")
                    .to_string_lossy();
                let inner_dir = inner_dir.split('.').next().expect("file has no extension?");
                let linux_files = output.join(inner_dir);
                for file in linux_files.read_dir()? {
                    let file = file?;
                    if file.path().is_file() {
                        fs::rename(
                            file.path(),
                            file.path().to_string_lossy().replace(inner_dir, ""),
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

/// Checks for updates to repak, downloads new binary zip if needed and extracts it. Copies the repak-gui binary to the cwd.
async fn update_repak(
    client: &Client,
    current_dir: &Path,
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

    let download_and_extract = async {
        let Some(downloads) = find_downloads(remote.assets) else {
            return Err(anyhow!("could not find download url"));
        };

        let download_start = Instant::now();

        let mut stdout = stdout().lock();
        writeln!(stdout, "\nstarting downloads\n")?;

        // clear the download path
        if download_path.exists() {
            fs::remove_dir_all(download_path)?;
        }
        fs::create_dir(download_path)?;

        for download in downloads {
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

            write!(stdout, "extracting..")?;
            stdout.flush()?;
            if let Err(err) = extract_archive(&download_output, download_path) {
                println!("failed to extract, errors: {}\n", get_error_chain(&err));
            } else {
                writeln!(stdout, "extracted.\n")?;
            }

            let repak_gui_path = if std::env::consts::OS == "windows" {
                "repak-gui.exe"
            } else {
                "repak-gui"
            };

            if fs::exists(repak_gui_path).is_ok_and(|exist| exist) {
                fs::remove_file(repak_gui_path)?;
            }

            fs::copy(
                download_path.join(repak_gui_path),
                current_dir.join(repak_gui_path),
            )?;
        }

        // drop stdout lock after we're done
        drop(stdout);

        Ok(())
    };

    match local_version {
        Ok(local_version) => {
            if remote_version > local_version {
                println!("found new version {remote_version}");
                download_and_extract.await?;
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
            download_and_extract.await?;
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

    let update_repak = async {
        // we are ok with update check failing
        if let Err(err) = update_repak(
            &Client::new(),
            current_dir,
            &download_path,
            &binary_path,
            &checked_marker_path,
        )
        .await
        {
            println!("failed to check for updates!");
            println!("errors: {}", get_error_chain(&err));
        }

        println!();

        Ok::<(), anyhow::Error>(())
    };

    let args = env::args().skip(1).collect::<Vec<_>>();
    // force update
    let args = if args.first().is_some_and(|a| a == "-U") {
        println!("forcing update checks.");
        update_repak.await?;
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
            Some(_) => update_repak.await?,
            // `CHECKED_MARKER` doesn't exist or something went wrong getting modified time.
            None => update_repak.await?,
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
