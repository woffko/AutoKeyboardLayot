//! Selected, pinned artifact transport. No installation or implicit selection.
//! Call the Windows transport only on a bounded background worker, never a hook.

use crate::{
    language_package::{PackageTrust, VerifiedLanguagePackage},
    package_catalog::{PinnedPackage, ReleaseError},
};
use std::{fmt, sync::Arc};

#[cfg(windows)]
mod winhttp;
#[cfg(windows)]
pub(crate) use winhttp::catalog_bytes;
#[cfg(windows)]
pub use winhttp::download;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadError {
    UrlPolicy,
    RedirectLimit,
    HttpStatus(u32),
    Network(i32),
    Header,
    Length,
    Cancelled,
    Deadline,
    Clock,
    Verification(ReleaseError),
}
impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never include redirect URLs, signed query parameters or response data.
        write!(f, "package_download_{self:?}")
    }
}
impl std::error::Error for DownloadError {}
impl From<ReleaseError> for DownloadError {
    fn from(error: ReleaseError) -> Self {
        Self::Verification(error)
    }
}

/// Exact transport/cache bytes authenticated against their selected catalog
/// entry. The manager must still stage the whole plan and recheck it at commit.
pub struct DownloadedPackage {
    bytes: Vec<u8>,
    package: Arc<VerifiedLanguagePackage>,
}
impl DownloadedPackage {
    pub fn from_bytes(
        pin: &PinnedPackage,
        bytes: Vec<u8>,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<Self, DownloadError> {
        let package = Arc::new(pin.verify_download(&bytes, trust, now)?);
        Ok(Self { bytes, package })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn package(&self) -> &Arc<VerifiedLanguagePackage> {
        &self.package
    }
}

#[cfg(any(windows, test))]
struct Target {
    host: String,
    path: String,
}
#[cfg(any(windows, test))]
impl Target {
    fn parse(url: &str) -> Result<Self, DownloadError> {
        if url.len() > 8192
            || !url.is_ascii()
            || url
                .bytes()
                .any(|c| c <= b' ' || c == 127 || c == b'\\' || c == b'#')
        {
            return Err(DownloadError::UrlPolicy);
        }
        let rest = url
            .strip_prefix("https://")
            .ok_or(DownloadError::UrlPolicy)?;
        let split = rest.find('/').ok_or(DownloadError::UrlPolicy)?;
        let host = rest[..split].to_ascii_lowercase();
        // Exact hosts, not a suffix wildcard or an implicit CDN/IP expansion.
        if !matches!(
            host.as_str(),
            "github.com" | "release-assets.githubusercontent.com"
        ) {
            return Err(DownloadError::UrlPolicy);
        }
        Ok(Self {
            host,
            path: rest[split..].to_owned(),
        })
    }
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
enum BodySize {
    Exact(u64),
    AtMost(u64),
}
#[cfg(any(windows, test))]
fn read_sized_body(
    size: BodySize,
    mut check: impl FnMut() -> Result<(), DownloadError>,
    mut read: impl FnMut(&mut [u8]) -> Result<usize, DownloadError>,
) -> Result<Vec<u8>, DownloadError> {
    let expected = match size {
        BodySize::Exact(bytes) | BodySize::AtMost(bytes) => bytes,
    };
    if expected == 0 || expected > crate::language_package::MAX_PACKAGE_BYTES as u64 {
        return Err(DownloadError::Length);
    }
    let mut bytes = Vec::with_capacity(expected.min(1024 * 1024) as usize);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        check()?;
        // One extra byte distinguishes an exact body from a malicious overrun.
        let capacity = ((expected - bytes.len() as u64 + 1) as usize).min(chunk.len());
        let count = read(&mut chunk[..capacity])?;
        check()?;
        if count > capacity || bytes.len() as u64 + count as u64 > expected {
            return Err(DownloadError::Length);
        }
        if count == 0 {
            let complete = match size {
                BodySize::Exact(_) => bytes.len() as u64 == expected,
                BodySize::AtMost(_) => !bytes.is_empty(),
            };
            return if complete {
                Ok(bytes)
            } else {
                Err(DownloadError::Length)
            };
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn read_body(
        expected: u64,
        check: impl FnMut() -> Result<(), DownloadError>,
        read: impl FnMut(&mut [u8]) -> Result<usize, DownloadError>,
    ) -> Result<Vec<u8>, DownloadError> {
        read_sized_body(BodySize::Exact(expected), check, read)
    }

    #[test]
    fn bounded_catalog_body_can_be_shorter_but_not_empty_or_over_limit() {
        let mut input = &b"ab"[..];
        assert_eq!(
            read_sized_body(
                BodySize::AtMost(3),
                || Ok(()),
                |out| input.read(out).map_err(|_| DownloadError::Length)
            )
            .unwrap(),
            b"ab"
        );
        for value in [&b""[..], &b"abcd"[..]] {
            let mut input = value;
            assert_eq!(
                read_sized_body(
                    BodySize::AtMost(3),
                    || Ok(()),
                    |out| input.read(out).map_err(|_| DownloadError::Length)
                ),
                Err(DownloadError::Length)
            );
        }
    }

    #[test]
    fn redirect_targets_allow_only_exact_https_release_hosts() {
        let target = Target::parse(
            "https://release-assets.githubusercontent.com/path/asset?sig=test%2Fvalue",
        )
        .unwrap();
        assert_eq!(target.host, "release-assets.githubusercontent.com");
        assert_eq!(target.path, "/path/asset?sig=test%2Fvalue");
        assert_eq!(
            Target::parse("https://GitHub.com/o/r/releases/download/r1/a.aklp")
                .unwrap()
                .host,
            "github.com"
        );
        for value in [
            "http://github.com/x",
            "https://github.com.evil.test/x",
            "https://evil@github.com/x",
            "https://github.com@evil.test/x",
            "https://github.com:443/x",
            "https://127.0.0.1/x",
            "https://github.com./x",
            "https://github.com/x#fragment",
            "https://github.com/\\evil",
            "https://github.com/x\r\nCookie:x",
            "https://github.com/пакет",
            "//github.com/x",
            "/relative",
        ] {
            assert!(Target::parse(value).is_err(), "{value:?}");
        }
        assert!(Target::parse(&format!("https://github.com/{}", "x".repeat(8192))).is_err());
    }

    #[test]
    fn body_reader_requires_exact_bytes_and_never_returns_a_partial_artifact() {
        let mut input = &b"abc"[..];
        assert_eq!(
            read_body(
                3,
                || Ok(()),
                |out| input.read(out).map_err(|_| DownloadError::Length)
            )
            .unwrap(),
            b"abc"
        );
        for value in [&b"ab"[..], &b"abcd"[..]] {
            let mut input = value;
            assert_eq!(
                read_body(
                    3,
                    || Ok(()),
                    |out| input.read(out).map_err(|_| DownloadError::Length)
                ),
                Err(DownloadError::Length)
            );
        }
        assert_eq!(
            read_body(1, || Ok(()), |out| Ok(out.len() + 1)),
            Err(DownloadError::Length)
        );
        assert_eq!(
            read_body(0, || Ok(()), |_| panic!("invalid limit must not read")),
            Err(DownloadError::Length)
        );
    }

    #[test]
    fn cancellation_and_deadline_checks_surround_each_body_read() {
        let cancelled = std::cell::Cell::new(false);
        let result = read_body(
            3,
            || {
                if cancelled.get() {
                    Err(DownloadError::Cancelled)
                } else {
                    Ok(())
                }
            },
            |out| {
                out[0] = b'a';
                cancelled.set(true);
                Ok(1)
            },
        );
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert_eq!(
            read_body(
                3,
                || Err(DownloadError::Deadline),
                |_| panic!("expired operation must not read")
            ),
            Err(DownloadError::Deadline)
        );
    }
}
