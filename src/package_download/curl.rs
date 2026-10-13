//! The Linux transport: the system's `curl`, run once per request with redirects disabled, so the
//! policy of the Windows transport holds here too: HTTPS with TLS 1.2 or newer only, exactly
//! github.com and its release-asset host, every redirect checked before it is followed (at most
//! three), an exact or bounded body, a deadline and cancellation. `curl -q` ignores `~/.curlrc`;
//! no credentials, cookies or `.netrc` are used and no compression is requested. A proxy set in the
//! environment is honoured, as the Windows transport honours the system proxy.

use super::{BodySize, DownloadError, DownloadedPackage, Target, read_sized_body};
use crate::{language_package::PackageTrust, package_catalog::PinnedPackage};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CURL: &str = "curl";

fn now() -> Result<u64, DownloadError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| DownloadError::Clock)
}

struct Budget<'a> {
    end: Instant,
    cancel: &'a AtomicBool,
}

impl Budget<'_> {
    fn check(&self) -> Result<(), DownloadError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        if Instant::now() >= self.end {
            return Err(DownloadError::Deadline);
        }
        Ok(())
    }

    fn remaining_seconds(&self) -> u64 {
        self.end
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1)
    }
}

/// Downloads exactly one selected catalog record; see the Windows transport for the contract.
pub fn download(
    pin: &PinnedPackage,
    trust: &PackageTrust,
    cancel: &AtomicBool,
) -> Result<DownloadedPackage, DownloadError> {
    pin.check_current(now()?)?;
    let budget = Budget {
        end: Instant::now() + Duration::from_secs(180),
        cancel,
    };
    let bytes = get(
        Path::new(CURL),
        pin.url(),
        BodySize::Exact(pin.bytes()),
        &budget,
    )?;
    let downloaded = DownloadedPackage::from_bytes(pin, bytes, trust, now()?)?;
    budget.check()?;
    pin.check_current(now()?)?;
    Ok(downloaded)
}

/// The latest catalog of `repository`, unauthenticated; it must enter catalog verification next.
pub(crate) fn catalog_bytes(
    repository: &str,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, DownloadError> {
    let repository = crate::package_catalog::repository_id(repository)?;
    let budget = Budget {
        end: Instant::now() + Duration::from_secs(180),
        cancel,
    };
    let url = format!("https://github.com/{repository}/releases/latest/download/catalog.aklc");
    get(
        Path::new(CURL),
        &url,
        BodySize::AtMost(crate::package_catalog::MAX_CATALOG_BYTES as u64),
        &budget,
    )
}

/// A response of one curl run: the status and, for a redirect, the target.
#[derive(Debug, PartialEq, Eq)]
struct Response {
    status: u32,
    location: Option<String>,
}

/// curl prints `%{http_code}` and `%{redirect_url}` on two lines.
fn parse_write_out(text: &str) -> Result<Response, DownloadError> {
    let mut lines = text.lines();
    let status = lines
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok())
        .ok_or(DownloadError::Header)?;
    let location = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned);
    Ok(Response { status, location })
}

/// A file only this process writes, removed when dropped.
struct BodyFile(PathBuf);

impl BodyFile {
    fn create() -> Result<Self, DownloadError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(std::env::temp_dir);
        let path = directory.join(format!(
            "autokeyboardlayot-download-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        options.open(&path).map_err(|_| DownloadError::LocalRead)?;
        Ok(Self(path))
    }
}

impl Drop for BodyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Runs one request without following redirects; the body goes to `body`.
fn request(
    program: &Path,
    target: &Target,
    limit: u64,
    body: &BodyFile,
    budget: &Budget<'_>,
) -> Result<Response, DownloadError> {
    budget.check()?;
    let url = format!("https://{}{}", target.host, target.path);
    let mut child = Command::new(program)
        .args([
            "-q",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-redirs",
            "0",
        ])
        .args(["--connect-timeout", "20", "--max-time"])
        .arg(budget.remaining_seconds().min(180).to_string())
        .arg("--max-filesize")
        .arg(limit.saturating_add(1).to_string())
        .args(["--user-agent", "AutoKeyboardLayot/package-download"])
        .args(["--write-out", "%{http_code}\n%{redirect_url}\n", "--output"])
        .arg(&body.0)
        .arg("--url")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| DownloadError::Network(-1))?;
    let status = loop {
        if let Err(error) = budget.check() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return Err(DownloadError::Network(-1)),
        }
    };
    let mut written = String::new();
    if let Some(stdout) = child.stdout.take() {
        stdout
            .take(4096)
            .read_to_string(&mut written)
            .map_err(|_| DownloadError::Header)?;
    }
    match status.code() {
        Some(0) => parse_write_out(&written),
        // curl: the body would exceed --max-filesize.
        Some(63) => Err(DownloadError::Length),
        Some(28) => Err(DownloadError::Deadline),
        Some(code) => Err(DownloadError::Network(code)),
        None => Err(DownloadError::Network(-1)),
    }
}

fn get(
    program: &Path,
    url: &str,
    body_size: BodySize,
    budget: &Budget<'_>,
) -> Result<Vec<u8>, DownloadError> {
    budget.check()?;
    let mut target = Target::parse(url)?;
    if target.host != "github.com" {
        return Err(DownloadError::UrlPolicy);
    }
    let limit = match body_size {
        BodySize::Exact(bytes) | BodySize::AtMost(bytes) => bytes,
    };
    let mut visited = BTreeSet::new();
    for hop in 0..=3 {
        if !visited.insert((target.host.clone(), target.path.clone())) {
            return Err(DownloadError::RedirectLimit);
        }
        let body = BodyFile::create()?;
        let response = request(program, &target, limit, &body, budget)?;
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            if hop == 3 {
                return Err(DownloadError::RedirectLimit);
            }
            target = Target::parse(response.location.as_deref().ok_or(DownloadError::Header)?)?;
            continue;
        }
        if response.status != 200 {
            return Err(DownloadError::HttpStatus(response.status));
        }
        let mut file = fs::File::open(&body.0).map_err(|_| DownloadError::LocalRead)?;
        let bytes = read_sized_body(
            body_size,
            || budget.check(),
            |out| file.read(out).map_err(|_| DownloadError::LocalRead),
        )?;
        budget.check()?;
        return Ok(bytes);
    }
    Err(DownloadError::RedirectLimit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in for curl: it answers each run with the next scripted response, writing the body
    /// to the --output path and the status line to stdout, and logs the URL it was given.
    fn fake_curl(directory: &Path, responses: &[(&str, &str, &str)]) -> PathBuf {
        let mut script = String::from(
            "#!/bin/sh\nout=''\nurl=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=$2; shift ;;\n    --url) url=$2; shift ;;\n  esac\n  shift\ndone\n",
        );
        script.push_str(&format!(
            "echo \"$url\" >> '{}'\n",
            directory.join("urls").display()
        ));
        script.push_str(&format!(
            "n=$(wc -l < '{}')\ncase $n in\n",
            directory.join("urls").display()
        ));
        for (index, (status, location, body)) in responses.iter().enumerate() {
            script.push_str(&format!(
                "  {}) printf '%s' '{body}' > \"$out\"; printf '{status}\\n{location}\\n' ;;\n",
                index + 1
            ));
        }
        script.push_str("esac\n");
        let path = directory.join("curl");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn run(
        responses: &[(&str, &str, &str)],
        url: &str,
        size: BodySize,
    ) -> (Result<Vec<u8>, DownloadError>, Vec<String>) {
        let _children = crate::test_support::spawning_children();
        let directory = tempfile::tempdir().unwrap();
        let program = fake_curl(directory.path(), responses);
        let cancel = AtomicBool::new(false);
        let budget = Budget {
            end: Instant::now() + Duration::from_secs(30),
            cancel: &cancel,
        };
        let result = get(&program, url, size, &budget);
        let urls = fs::read_to_string(directory.path().join("urls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        (result, urls)
    }

    const ASSET: &str = "https://github.com/o/r/releases/download/t/a.aklp";
    const SIGNED: &str = "https://release-assets.githubusercontent.com/x/a.aklp?sig=1";

    #[test]
    fn a_redirect_to_the_asset_host_is_followed_and_the_exact_body_returned() {
        let (result, urls) = run(
            &[("302", SIGNED, ""), ("200", "", "abc")],
            ASSET,
            BodySize::Exact(3),
        );
        assert_eq!(result.unwrap(), b"abc");
        assert_eq!(urls, [ASSET, SIGNED]);
    }

    #[test]
    fn redirects_elsewhere_loops_and_wrong_sizes_are_refused() {
        let (result, urls) = run(
            &[("302", "https://evil.example/a", "")],
            ASSET,
            BodySize::Exact(3),
        );
        assert_eq!(result.unwrap_err(), DownloadError::UrlPolicy);
        assert_eq!(urls.len(), 1, "the foreign host is never contacted");
        let (result, _) = run(
            &[("302", "http://github.com/o/r/a", "")],
            ASSET,
            BodySize::Exact(3),
        );
        assert_eq!(result.unwrap_err(), DownloadError::UrlPolicy);
        let (result, _) = run(&[("302", ASSET, "")], ASSET, BodySize::Exact(3));
        assert_eq!(result.unwrap_err(), DownloadError::RedirectLimit);
        let (result, _) = run(&[("200", "", "ab")], ASSET, BodySize::Exact(3));
        assert_eq!(result.unwrap_err(), DownloadError::Length);
        let (result, _) = run(&[("404", "", "")], ASSET, BodySize::Exact(3));
        assert_eq!(result.unwrap_err(), DownloadError::HttpStatus(404));
        let (result, _) = run(&[("200", "", "ab")], ASSET, BodySize::AtMost(3));
        assert_eq!(result.unwrap(), b"ab");
        // Only github.com is a valid first host.
        let (result, urls) = run(&[("200", "", "abc")], SIGNED, BodySize::Exact(3));
        assert_eq!(result.unwrap_err(), DownloadError::UrlPolicy);
        assert!(urls.is_empty());
    }

    #[test]
    fn a_cancelled_request_stops_before_running_curl() {
        let directory = tempfile::tempdir().unwrap();
        let program = fake_curl(directory.path(), &[("200", "", "abc")]);
        let cancel = AtomicBool::new(true);
        let budget = Budget {
            end: Instant::now() + Duration::from_secs(30),
            cancel: &cancel,
        };
        assert_eq!(
            get(&program, ASSET, BodySize::Exact(3), &budget).unwrap_err(),
            DownloadError::Cancelled
        );
        assert!(!directory.path().join("urls").exists());
    }

    /// Reads the published catalog and downloads its smallest package from GitHub, through the
    /// redirect to the asset host: `cargo test --lib -- --ignored published`.
    #[test]
    #[ignore = "reads the published catalog and one package from GitHub"]
    fn the_published_catalog_and_a_package_download_and_verify() {
        use crate::package_catalog::{DEFAULT_PACKAGE_REPOSITORY, VerifiedReleaseCatalog};
        let _children = crate::test_support::spawning_children();
        let cancel = AtomicBool::new(false);
        let trust = PackageTrust::release().unwrap();
        let bytes = catalog_bytes(DEFAULT_PACKAGE_REPOSITORY, &cancel).unwrap();
        let now = now().unwrap();
        let catalog =
            VerifiedReleaseCatalog::verify(&bytes, &trust, DEFAULT_PACKAGE_REPOSITORY, None, now)
                .unwrap();
        assert!(catalog.expires_at() > now);
        let smallest = catalog.packages().min_by_key(|pin| pin.bytes()).unwrap();
        let downloaded = download(smallest, &trust, &cancel).unwrap();
        assert_eq!(downloaded.bytes().len() as u64, smallest.bytes());
    }

    #[test]
    fn the_write_out_lines_are_parsed() {
        assert_eq!(
            parse_write_out("302\nhttps://release-assets.githubusercontent.com/x\n").unwrap(),
            Response {
                status: 302,
                location: Some("https://release-assets.githubusercontent.com/x".into())
            }
        );
        assert_eq!(parse_write_out("200\n\n").unwrap().location, None);
        assert_eq!(parse_write_out("").unwrap_err(), DownloadError::Header);
    }
}
