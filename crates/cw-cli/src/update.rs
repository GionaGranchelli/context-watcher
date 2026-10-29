use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tar::Archive;
use uuid::Uuid;

const REPOSITORY: &str = "https://github.com/GionaGranchelli/context-watcher.git";
const RELEASE_API: &str =
    "https://api.github.com/repos/GionaGranchelli/context-watcher/releases/latest";
const MAX_RELEASE_ASSET_BYTES: usize = 200 * 1024 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub async fn run(from_source: bool, no_restart: bool) -> Result<()> {
    let temp_dir = StagingDir::new()?;

    let staged = if from_source {
        build_latest_main(temp_dir.path())?
    } else {
        download_release_binary(temp_dir.path()).await?
    };
    let installed = install_atomically(&staged)?;

    println!("Installed updated cw binary at {}", installed.display());
    run_installed(&installed, &["init", "--no-start"])
        .context("refresh cw harness integrations")?;
    if !no_restart {
        run_installed(&installed, &["daemon", "restart"])
            .context("restart cw daemon with the updated binary")?;
    } else {
        println!("Skipped daemon restart (--no-restart). Run `cw daemon restart` when ready.");
    }
    Ok(())
}

struct StagingDir(PathBuf);

impl StagingDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("cw-update-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).context("create update staging directory")?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn download_release_binary(temp_dir: &Path) -> Result<PathBuf> {
    let target = release_target()?;
    let asset_base = format!("cw-{target}");
    let client = reqwest::Client::builder()
        .user_agent(format!("cw-updater/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .build()?;
    let release: Release = client
        .get(RELEASE_API)
        .send()
        .await?
        .error_for_status()
        .context("fetch latest cw release metadata")?
        .json()
        .await
        .context("parse latest cw release metadata")?;
    let archive_asset = release_asset(&release, &format!("{asset_base}.tar.gz"))?;
    let checksum_asset = release_asset(&release, &format!("{asset_base}.tar.gz.sha256"))?;
    let archive = download_asset(&client, archive_asset).await?;
    let checksum = download_asset(&client, checksum_asset).await?;
    verify_sha256(&archive, &checksum)?;

    let binary_path = temp_dir.join("cw");
    extract_binary(&archive, &binary_path)?;
    println!(
        "Downloaded and verified cw release {} ({target}).",
        release.tag_name
    );
    Ok(binary_path)
}

fn release_asset<'a>(release: &'a Release, name: &str) -> Result<&'a ReleaseAsset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .with_context(|| format!("latest release does not contain required asset {name}"))
}

async fn download_asset(client: &reqwest::Client, asset: &ReleaseAsset) -> Result<Vec<u8>> {
    let url = reqwest::Url::parse(&asset.browser_download_url)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("github.com")
                    || host.eq_ignore_ascii_case("objects.githubusercontent.com")
            }),
        "release asset URL is not an HTTPS GitHub URL"
    );
    let mut response = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("download release asset {}", asset.name))?;
    if let Some(length) = response.content_length() {
        ensure!(
            length <= MAX_RELEASE_ASSET_BYTES as u64,
            "release asset {} exceeds the download size limit",
            asset.name
        );
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_RELEASE_ASSET_BYTES,
            "release asset {} exceeds the download size limit",
            asset.name
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn verify_sha256(archive: &[u8], checksum_file: &[u8]) -> Result<()> {
    let checksum = std::str::from_utf8(checksum_file).context("release checksum is not UTF-8")?;
    let expected = checksum
        .split_whitespace()
        .next()
        .context("release checksum is empty")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "release checksum is not a SHA-256 digest"
    );
    let actual = sha256_hex(archive);
    ensure!(
        actual.eq_ignore_ascii_case(expected),
        "release checksum mismatch"
    );
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn extract_binary(archive_bytes: &[u8], binary_path: &Path) -> Result<()> {
    let mut archive = Archive::new(GzDecoder::new(Cursor::new(archive_bytes)));
    let entries = archive.entries().context("read release archive")?;
    let mut found = false;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(binary_path)
        .context("create staged cw binary")?;
    for entry in entries {
        let mut entry = entry.context("read release archive entry")?;
        let path = entry.path().context("read release archive path")?;
        if path != Path::new("cw") {
            continue;
        }
        ensure!(!found, "release archive contains duplicate cw binaries");
        ensure!(
            entry.header().entry_type().is_file(),
            "release cw entry is not a regular file"
        );
        found = true;
        std::io::copy(&mut entry, &mut output).context("extract cw binary")?;
    }
    ensure!(found, "release archive has no top-level cw binary");
    output.sync_all().context("sync staged cw binary")?;
    Ok(())
}

fn build_latest_main(temp_dir: &Path) -> Result<PathBuf> {
    let source = temp_dir.join("source");
    let mut clone = Command::new("git");
    clone
        .args(["clone", "--depth=1", "--branch", "main", REPOSITORY])
        .arg(&source);
    run_command(&mut clone, temp_dir, "clone latest main branch")?;
    let web = source.join("web");
    run_command(
        Command::new("npm").args(["ci"]),
        &web,
        "install web build dependencies",
    )?;
    run_command(
        Command::new("npm").args(["run", "build"]),
        &web,
        "build embedded web assets",
    )?;
    run_command(
        Command::new("cargo").args(["build", "--locked", "--release", "-p", "cw-cli"]),
        &source,
        "build cw from latest main branch",
    )?;
    let binary = source.join("target/release/cw");
    ensure!(
        binary.is_file(),
        "source build did not produce target/release/cw"
    );
    Ok(binary)
}

fn run_command(command: &mut Command, cwd: &Path, action: &str) -> Result<()> {
    let status = command
        .current_dir(cwd)
        .status()
        .with_context(|| format!("{action}: could not start command"))?;
    ensure!(status.success(), "{action} failed with status {status}");
    Ok(())
}

fn install_atomically(source: &Path) -> Result<PathBuf> {
    let destination = cw_core::home_dir().join(".local/bin/cw");
    install_atomically_to(source, &destination)?;
    Ok(destination)
}

fn install_atomically_to(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("cw install path has no parent directory")?;
    fs::create_dir_all(parent).context("create ~/.local/bin")?;
    let staged = parent.join(format!(".cw-update-{}", Uuid::new_v4().simple()));
    let result = (|| {
        let mut input = File::open(source).context("open staged cw binary")?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .context("create same-directory install staging file")?;
        std::io::copy(&mut input, &mut output).context("copy updated cw binary")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output
                .set_permissions(fs::Permissions::from_mode(0o755))
                .context("set cw executable permissions")?;
        }
        output.sync_all().context("sync updated cw binary")?;
        fs::rename(&staged, destination).context("atomically replace ~/.local/bin/cw")?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .context("sync cw install directory")?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result?;
    Ok(())
}

fn run_installed(binary: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(binary)
        .args(args)
        .status()
        .with_context(|| format!("run {} {}", binary.display(), args.join(" ")))?;
    ensure!(
        status.success(),
        "{} {} failed with status {status}",
        binary.display(),
        args.join(" ")
    );
    Ok(())
}

fn release_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => bail!("no cw release binary for {os}/{arch}; try `cw update --from-source`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};

    #[test]
    fn verifies_release_checksum_and_rejects_mismatch() {
        let bytes = b"release archive";
        let digest = sha256_hex(bytes);
        assert!(verify_sha256(bytes, format!("{digest}  cw.tar.gz\n").as_bytes()).is_ok());
        assert!(
            verify_sha256(bytes, format!("{}  cw.tar.gz\n", "0".repeat(64)).as_bytes()).is_err()
        );
        assert!(verify_sha256(bytes, b"not-a-checksum").is_err());
    }

    #[test]
    fn release_target_matches_supported_release_matrix() {
        let target = release_target().unwrap();
        assert!(matches!(
            target,
            "x86_64-unknown-linux-gnu"
                | "aarch64-unknown-linux-gnu"
                | "x86_64-apple-darwin"
                | "aarch64-apple-darwin"
        ));
    }

    #[test]
    fn extracts_only_the_top_level_regular_cw_file() {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "cw", &b"binary"[..])
            .unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let dir = std::env::temp_dir().join(format!("cw-update-test-{}", Uuid::new_v4().simple()));
        fs::create_dir(&dir).unwrap();
        let binary = dir.join("staged-cw");

        assert!(extract_binary(&bytes, &binary).is_ok());
        assert_eq!(fs::read(&binary).unwrap(), b"binary");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomically_replaces_existing_binary() {
        let dir = StagingDir::new().unwrap();
        let source = dir.path().join("new-cw");
        let destination = dir.path().join("bin/cw");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&source, b"new binary").unwrap();
        fs::write(&destination, b"old binary").unwrap();

        assert!(install_atomically_to(&source, &destination).is_ok());
        assert_eq!(fs::read(destination).unwrap(), b"new binary");
    }
}
