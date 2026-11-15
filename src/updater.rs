//! User-initiated updates from the project's GitHub releases.
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const RELEASE_API: &str =
    "https://api.github.com/repos/Jcd1230/digiorno-stitchworks/releases/latest";
const RELEASE_URL: &str = "https://github.com/Jcd1230/digiorno-stitchworks/releases/download";
const STAGE_PREFIX: &str = ".designer1-update-";
const HELPER_ARG: &str = "--internal-apply-update";
const CLEANUP_ENV: &str = "DESIGNER1_UPDATE_CLEANUP";
const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
const MAX_BINARY: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: Version,
    pub notes: String,
    url: String,
    digest: String,
    asset_name: String,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

fn asset_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("designer1-x86_64-pc-windows-msvc.zip"),
        ("linux", "x86_64") => Ok("designer1-x86_64-unknown-linux-gnu.tar.gz"),
        _ => {
            bail!("In-app updates are available for the official x86-64 Windows and Linux builds.")
        }
    }
}
fn executable_name() -> &'static str {
    if cfg!(windows) {
        "designer1.exe"
    } else {
        "designer1"
    }
}
fn client() -> ureq::Agent {
    let builder = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(120)));
    #[cfg(windows)]
    let builder = builder.tls_config(
        ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build(),
    );
    builder.build().into()
}

fn parse_release(
    bytes: &[u8],
    current: &Version,
    expected_asset: &str,
) -> Result<Option<UpdateInfo>> {
    let release: Release =
        serde_json::from_slice(bytes).context("reading GitHub release information")?;
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .context("the latest release has an invalid version")?;
    if version <= *current {
        return Ok(None);
    }
    ensure!(version.pre.is_empty(), "the latest release is a prerelease");
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == expected_asset)
        .context("the release does not yet contain an update for this platform; try again later")?;
    let expected_url = format!("{RELEASE_URL}/{}/{expected_asset}", release.tag_name);
    ensure!(
        asset.browser_download_url == expected_url,
        "unexpected update download URL"
    );
    let digest = asset
        .digest
        .context("GitHub did not provide an update checksum")?;
    let hash = digest
        .strip_prefix("sha256:")
        .context("unsupported update checksum")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid update checksum"
    );
    Ok(Some(UpdateInfo {
        version,
        notes: release.body.unwrap_or_default(),
        url: expected_url,
        digest: hash.to_ascii_lowercase(),
        asset_name: expected_asset.to_owned(),
    }))
}
pub fn check() -> Result<Option<UpdateInfo>> {
    let name = asset_name()?;
    let mut response = client()
        .get(RELEASE_API)
        .header(
            "User-Agent",
            concat!("designer1/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("checking GitHub for updates")?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "GitHub release information is too large"
    );
    parse_release(&bytes, &Version::parse(env!("CARGO_PKG_VERSION"))?, name)
}
fn executable_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running application")?;
    ensure!(
        exe.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(executable_name())),
        "Updates are available for the official designer1 executable. Download the release manually for other builds."
    );
    Ok(exe)
}
pub struct PreparedUpdate {
    stage: tempfile::TempDir,
    target: PathBuf,
}
fn read_limited(reader: &mut dyn Read, limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.take(limit + 1).read_to_end(&mut out)?;
    ensure!(
        !out.is_empty() && out.len() as u64 <= limit,
        "update file is empty or too large"
    );
    Ok(out)
}
fn extract_binary(archive: &[u8], name: &str, is_zip: bool) -> Result<Vec<u8>> {
    let mut found = None;
    if is_zip {
        let mut zip =
            zip::ZipArchive::new(io::Cursor::new(archive)).context("opening the update ZIP")?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            if entry.name() != name {
                continue;
            }
            ensure!(
                found.is_none()
                    && entry.is_file()
                    && entry
                        .unix_mode()
                        .is_none_or(|mode| mode & 0o170000 != 0o120000),
                "invalid or duplicate executable in update"
            );
            found = Some(read_limited(&mut entry, MAX_BINARY)?);
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        for entry in tar.entries().context("opening the update archive")? {
            let mut entry = entry?;
            if entry.path()?.as_ref() != Path::new(name) {
                continue;
            }
            ensure!(
                found.is_none() && entry.header().entry_type().is_file(),
                "invalid or duplicate executable in update"
            );
            found = Some(read_limited(&mut entry, MAX_BINARY)?);
        }
    }
    found.context("the update archive does not contain the expected executable")
}
fn verify_checksum(archive: &[u8], expected: &str) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(archive));
    ensure!(
        actual == expected,
        "download checksum did not match GitHub; the update was not installed"
    );
    Ok(())
}

pub fn prepare(info: &UpdateInfo) -> Result<PreparedUpdate> {
    let target = executable_path()?;
    let parent = target
        .parent()
        .context("application has no installation directory")?;
    let stage = tempfile::Builder::new().prefix(STAGE_PREFIX).tempdir_in(parent).with_context(||
        format!("cannot write to {}; install the update manually or run the app with permission to modify this folder", parent.display()))?;
    let mut response = client()
        .get(&info.url)
        .header(
            "User-Agent",
            concat!("designer1/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .context("downloading the update")?;
    let archive = read_limited(&mut response.body_mut().as_reader(), MAX_ARCHIVE)?;
    verify_checksum(&archive, &info.digest)?;
    let binary = extract_binary(
        &archive,
        executable_name(),
        info.asset_name.ends_with(".zip"),
    )?;
    let expected_magic: &[u8] = if cfg!(windows) { b"MZ" } else { b"\x7fELF" };
    ensure!(
        binary.starts_with(expected_magic),
        "the update archive contains an invalid executable"
    );
    let next = stage.path().join(executable_name());
    let mut file = File::create(&next)?;
    file.write_all(&binary)?;
    file.sync_all()?;
    fs::set_permissions(&next, fs::metadata(&target)?.permissions())?;
    fs::copy(&target, stage.path().join(helper_name())).context("preparing the update helper")?;
    Ok(PreparedUpdate { stage, target })
}
fn helper_name() -> &'static str {
    if cfg!(windows) {
        "update-helper.exe"
    } else {
        "update-helper"
    }
}
pub fn launch_helper(prepared: PreparedUpdate) -> Result<Child> {
    let helper = prepared.stage.path().join(helper_name());
    let mut command = Command::new(&helper);
    command
        .arg(HELPER_ARG)
        .arg(&prepared.target)
        .arg(prepared.stage.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command.spawn().context("starting the update helper")?;
    let _ = prepared.stage.keep();
    Ok(child)
}
fn apply_update(target: &Path, stage: &Path) -> Result<()> {
    validate_stage(target, stage)?;
    let next = stage.join(executable_name());
    let backup = stage.join("previous-executable");
    ensure!(
        next.is_file() && !backup.exists(),
        "update files are missing or invalid"
    );
    // The parent's pipe closes on exit. Windows can keep the image mapped briefly afterward.
    let mut last = None;
    for _ in 0..100 {
        match fs::rename(target, &backup) {
            Ok(()) => {
                last = None;
                break;
            }
            Err(err) => {
                last = Some(err);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    if let Some(err) = last {
        return Err(err).context("the running application could not be replaced");
    }
    if let Err(err) = fs::rename(&next, target) {
        let restored = fs::rename(&backup, target);
        if let Err(restore) = restored {
            bail!(
                "could not install update ({err}) or restore the previous executable ({restore}); backup: {}",
                backup.display()
            );
        }
        return Err(err)
            .context("installing the downloaded executable; the previous version was restored");
    }
    if let Err(err) = Command::new(target)
        .arg("--gui")
        .env(CLEANUP_ENV, stage)
        .spawn()
    {
        if let Err(remove_error) = fs::remove_file(target) {
            bail!(
                "the updated application could not start ({err}) and could not be removed ({remove_error}); the previous executable is at {}",
                backup.display()
            );
        }
        if let Err(restore_error) = fs::rename(&backup, target) {
            bail!(
                "the updated application could not start ({err}) and the previous executable could not be restored ({restore_error}); backup: {}",
                backup.display()
            );
        }
        return Err(err)
            .context("starting the updated application; the previous version was restored");
    }
    Ok(())
}
fn validate_stage(target: &Path, stage: &Path) -> Result<()> {
    ensure!(
        target
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(executable_name())),
        "invalid application path"
    );
    ensure!(
        stage.parent() == target.parent()
            && stage
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(STAGE_PREFIX)),
        "invalid update directory"
    );
    ensure!(
        stage.is_dir() && !fs::symlink_metadata(stage)?.file_type().is_symlink(),
        "invalid update directory"
    );
    Ok(())
}
pub fn handle_helper_args(args: &[OsString]) -> bool {
    if args.get(1).and_then(|s| s.to_str()) != Some(HELPER_ARG) {
        return false;
    }
    let result = (|| -> Result<()> {
        ensure!(args.len() == 4, "invalid update helper arguments");
        // Block until the GUI process closes its pipe.
        io::stdin().read_to_end(&mut Vec::new())?;
        apply_update(Path::new(&args[2]), Path::new(&args[3]))
    })();
    if let Err(err) = result {
        rfd::MessageDialog::new()
            .set_title("Designer 1 update failed")
            .set_description(format!(
                "{err:#}\n\nPlease start Designer 1 again or install the release manually."
            ))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
    true
}
pub fn cleanup_previous_update() {
    let Some(stage) = std::env::var_os(CLEANUP_ENV).map(PathBuf::from) else {
        return;
    };
    let Ok(target) = executable_path() else {
        return;
    };
    if validate_stage(&target, &stage).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..30 {
            if fs::remove_dir_all(&stage).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn release(tag: &str, name: &str, digest: Option<&str>, url: &str) -> Vec<u8> {
        serde_json::json!({ "tag_name": tag, "body": "Changes", "assets": [{
            "name": name, "digest": digest, "browser_download_url": url
        }] })
        .to_string()
        .into_bytes()
    }
    #[test]
    fn selects_only_newer_release_and_expected_asset() {
        let name = "designer1-x86_64-pc-windows-msvc.zip";
        let url = format!("{RELEASE_URL}/v1.2.4/{name}");
        let checksum = format!("sha256:{}", "a".repeat(64));
        let data = release("v1.2.4", name, Some(&checksum), &url);
        let info = parse_release(&data, &Version::parse("1.2.3").unwrap(), name)
            .unwrap()
            .unwrap();
        assert_eq!(info.version, Version::parse("1.2.4").unwrap());
        assert_eq!(info.notes, "Changes");
        assert!(
            parse_release(&data, &Version::parse("1.2.4").unwrap(), name)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_release(&data, &Version::parse("1.3.0").unwrap(), name)
                .unwrap()
                .is_none()
        );
        assert!(parse_release(&data, &Version::parse("1.2.3").unwrap(), "other.zip").is_err());
        assert!(
            parse_release(
                &release("v1.2.4", name, None, &url),
                &Version::parse("1.2.3").unwrap(),
                name
            )
            .is_err()
        );
        assert!(
            parse_release(
                &release(
                    "v1.2.4",
                    name,
                    Some(&checksum),
                    "https://example.com/update.zip"
                ),
                &Version::parse("1.2.3").unwrap(),
                name
            )
            .is_err()
        );
        assert!(
            parse_release(
                &release("v1.2.4", name, Some("sha256:bad"), &url),
                &Version::parse("1.2.3").unwrap(),
                name
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_checksum_mismatch() {
        let expected = format!("{:x}", Sha256::digest(b"correct"));
        assert!(verify_checksum(b"correct", &expected).is_ok());
        assert!(verify_checksum(b"wrong", &expected).is_err());
    }
    #[test]
    fn extracts_exact_binary_from_zip_and_tar() {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("README.md", options).unwrap();
        zip.write_all(b"notes").unwrap();
        zip.start_file("designer1.exe", options).unwrap();
        zip.write_all(b"windows binary").unwrap();
        let zip = zip.finish().unwrap().into_inner();
        assert_eq!(
            extract_binary(&zip, "designer1.exe", true).unwrap(),
            b"windows binary"
        );
        assert!(extract_binary(&zip, "designer1", true).is_err());
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(12);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "designer1", &b"linux binary"[..])
            .unwrap();
        let tar = tar.into_inner().unwrap().finish().unwrap();
        assert_eq!(
            extract_binary(&tar, "designer1", false).unwrap(),
            b"linux binary"
        );
        assert!(extract_binary(&tar, "designer1.exe", false).is_err());
        assert!(extract_binary(b"not an archive", "designer1", false).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn replacement_restores_old_binary_if_restart_fails() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let stage = tempfile::Builder::new()
            .prefix(STAGE_PREFIX)
            .tempdir_in(parent.path())
            .unwrap();
        let target = parent.path().join(executable_name());
        fs::write(&target, b"old executable").unwrap();
        fs::write(stage.path().join(executable_name()), b"invalid executable").unwrap();
        fs::set_permissions(
            stage.path().join(executable_name()),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let error = apply_update(&target, stage.path()).unwrap_err();
        assert!(format!("{error:#}").contains("previous version was restored"));
        assert_eq!(fs::read(&target).unwrap(), b"old executable");
    }
    #[test]
    fn rejects_staging_directories_outside_installation() {
        let parent = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        assert!(validate_stage(&parent.path().join(executable_name()), stage.path()).is_err());
    }
    #[test]
    #[ignore = "downloads release assets from GitHub"]
    fn live_github_release_archives() {
        let mut response = client()
            .get(RELEASE_API)
            .header("User-Agent", "designer1-update-test")
            .call()
            .unwrap();
        let body = read_limited(&mut response.body_mut().as_reader(), 1024 * 1024).unwrap();
        for (asset, name, zip) in [
            (
                "designer1-x86_64-unknown-linux-gnu.tar.gz",
                "designer1",
                false,
            ),
            (
                "designer1-x86_64-pc-windows-msvc.zip",
                "designer1.exe",
                true,
            ),
        ] {
            let info = parse_release(&body, &Version::new(0, 0, 0), asset)
                .unwrap()
                .unwrap();
            let mut response = client()
                .get(&info.url)
                .header("User-Agent", "designer1-update-test")
                .call()
                .unwrap();
            let archive = read_limited(&mut response.body_mut().as_reader(), MAX_ARCHIVE).unwrap();
            verify_checksum(&archive, &info.digest).unwrap();
            let binary = extract_binary(&archive, name, zip).unwrap();
            if zip {
                assert!(binary.starts_with(b"MZ"));
            } else {
                assert!(binary.starts_with(b"\x7fELF"));
            }
        }
    }

    #[test]
    #[ignore = "requires network access to GitHub"]
    fn live_github_release_check() {
        check().expect("GitHub release request should succeed");
    }
}
