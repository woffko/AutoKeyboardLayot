//! Synchronous WinHTTP confined to a caller-owned background worker. Handles
//! never cross threads; cancellation is checked between bounded blocking calls.

use super::{BodySize, DownloadError, DownloadedPackage, Target, read_sized_body};
use crate::{language_package::PackageTrust, package_catalog::PinnedPackage};
use std::{
    collections::BTreeSet,
    ffi::c_void,
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::Networking::WinHttp::*,
    core::{HSTRING, PCWSTR, w},
};

struct Handle(*mut c_void);
impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, DownloadError> {
        if raw.is_null() {
            Err(network(windows::core::Error::from_thread()))
        } else {
            Ok(Self(raw))
        }
    }
    fn option(&self, option: u32, value: u32) -> Result<(), DownloadError> {
        unsafe {
            WinHttpSetOption(
                Some(self.0.cast_const()),
                option,
                Some(&value.to_ne_bytes()),
            )
        }
        .map_err(network)
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}
fn network(error: windows::core::Error) -> DownloadError {
    DownloadError::Network(error.code().0)
}
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
    fn timeouts(&self, handle: &Handle) -> Result<(), DownloadError> {
        self.check()?;
        let remaining = self
            .end
            .saturating_duration_since(Instant::now())
            .as_millis()
            .clamp(1, 20_000) as i32;
        unsafe {
            WinHttpSetTimeouts(
                handle.0,
                remaining.min(5000),
                remaining.min(10_000),
                remaining.min(10_000),
                remaining,
            )
        }
        .map_err(network)?;
        handle.option(WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, remaining as u32)
    }
}

/// Download exactly one already selected catalog record. No automatic retry,
/// dependency download, cache/config writes, installation or global settings.
/// Uses Windows system/per-user proxy selection but never supplies credentials;
/// automatic request authentication/cookies and redirects are disabled.
/// Cancellation/deadline checks surround blocking calls, not a hard real-time
/// guarantee: OS/proxy operations may take up to their configured timeouts.
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
    let bytes = get(pin.url(), BodySize::Exact(pin.bytes()), &budget)?;
    let downloaded = DownloadedPackage::from_bytes(pin, bytes, trust, now()?)?;
    budget.check()?;
    pin.check_current(now()?)?;
    Ok(downloaded)
}

/// Unauthenticated metadata bytes are crate-private and must immediately enter
/// PreparedCatalog verification. No arbitrary caller URL or authentication data.
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
        &url,
        BodySize::AtMost(crate::package_catalog::MAX_CATALOG_BYTES as u64),
        &budget,
    )
}

fn get(url: &str, body_size: BodySize, budget: &Budget<'_>) -> Result<Vec<u8>, DownloadError> {
    budget.check()?;
    let mut target = Target::parse(url)?;
    if target.host != "github.com" {
        return Err(DownloadError::UrlPolicy);
    }
    let session = Handle::new(unsafe {
        WinHttpOpen(
            w!("AutoKeyboardLayot/package-download"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    session.option(
        WINHTTP_OPTION_SECURE_PROTOCOLS,
        WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
    )?;
    session.option(WINHTTP_OPTION_CONNECT_RETRIES, 2)?;
    session.option(WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE, 16 * 1024)?;
    session.option(WINHTTP_OPTION_MAX_HTTP_STATUS_CONTINUE, 3)?;
    budget.timeouts(&session)?;
    let mut visited = BTreeSet::new();
    for hop in 0..=3 {
        budget.check()?;
        if !visited.insert((target.host.clone(), target.path.clone())) {
            return Err(DownloadError::RedirectLimit);
        }
        let host = HSTRING::from(target.host.as_str());
        let connection = Handle::new(unsafe { WinHttpConnect(session.0, &host, 443, 0) })?;
        let path = HSTRING::from(target.path.as_str());
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                &path,
                PCWSTR::null(),
                PCWSTR::null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;
        request.option(
            WINHTTP_OPTION_DISABLE_FEATURE,
            WINHTTP_DISABLE_AUTHENTICATION
                | WINHTTP_DISABLE_COOKIES
                | WINHTTP_DISABLE_REDIRECTS
                | WINHTTP_DISABLE_KEEP_ALIVE,
        )?;
        request.option(
            WINHTTP_OPTION_AUTOLOGON_POLICY,
            WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
        )?;
        budget.timeouts(&request)?;
        let headers: Vec<u16> = "Accept-Encoding: identity\r\n".encode_utf16().collect();
        unsafe { WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0) }.map_err(network)?;
        budget.timeouts(&request)?;
        unsafe { WinHttpReceiveResponse(request.0, null_mut()) }.map_err(network)?;
        budget.check()?;
        let status = status(&request)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if hop == 3 {
                return Err(DownloadError::RedirectLimit);
            }
            target = Target::parse(&location(&request)?)?;
            continue;
        }
        if status != 200 {
            return Err(DownloadError::HttpStatus(status));
        }
        let bytes = read_sized_body(
            body_size,
            || budget.check(),
            |out| {
                budget.timeouts(&request)?;
                let mut count = 0;
                unsafe {
                    WinHttpReadData(
                        request.0,
                        out.as_mut_ptr().cast(),
                        out.len() as u32,
                        &mut count,
                    )
                }
                .map_err(network)?;
                Ok(count as usize)
            },
        )?;
        budget.check()?;
        return Ok(bytes);
    }
    Err(DownloadError::RedirectLimit)
}

fn status(request: &Handle) -> Result<u32, DownloadError> {
    let mut status = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut bytes,
            null_mut(),
        )
    }
    .map_err(network)?;
    if bytes != size_of::<u32>() as u32 {
        return Err(DownloadError::Header);
    }
    Ok(status)
}

fn location(request: &Handle) -> Result<String, DownloadError> {
    let mut text = vec![0u16; 4096];
    let mut bytes = (text.len() * size_of::<u16>()) as u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_LOCATION,
            PCWSTR::null(),
            Some(text.as_mut_ptr().cast()),
            &mut bytes,
            null_mut(),
        )
    }
    .map_err(|_| DownloadError::Header)?;
    if bytes as usize > text.len() * 2 || !bytes.is_multiple_of(2) {
        return Err(DownloadError::Header);
    }
    let units = &text[..bytes as usize / 2];
    let units = units.strip_suffix(&[0]).unwrap_or(units);
    String::from_utf16(units).map_err(|_| DownloadError::Header)
}
