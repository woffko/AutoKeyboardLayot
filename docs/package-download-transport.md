# Selected package transport (development)

`package_download::download` is a Windows-only background-worker API for one
`PinnedPackage`, not a catalog browser, downloader queue or installer. It never
chooses additional packages, writes files, changes configuration or installs an
artifact. The GUI must obtain explicit selection/confirmation and retain the
whole authenticated plan for the inventory transaction. The new
[online manager](online-package-manager.md) supplies those steps, with Windows
validation pending. Source policy now selects `woffko/AutoKeyboardLayot` and
embeds the public release key; published signed artifacts and live acceptance
remain pending.

## Source and response checks

The initial URL comes from the authenticated catalog and must use `github.com`.
Redirects are followed manually, at most three hops, with loop detection. Only
absolute HTTPS targets on exactly `github.com` or
`release-assets.githubusercontent.com` are allowed, not domain suffix matches,
IP addresses, userinfo, explicit ports, fragments, backslashes or raw controls.
Relative redirects and other CDN hosts fail closed. GitHub documents the latter
host for release downloads in its [network requirements](https://docs.github.com/en/actions/reference/runners/self-hosted-runners).

WinHTTP uses system/per-user proxy selection and TLS 1.2/1.3 with normal
certificate verification. Every request disables automatic authentication,
cookies, redirects and keep-alive; no credentials or client certificate are
supplied. Unsupported required options fail instead of weakening the policy.
Authentication-required servers/proxies are errors, not prompts or token lookup.
The options and handle types follow [Microsoft's WinHTTP option reference](https://learn.microsoft.com/en-us/windows/win32/winhttp/option-flags)
and [WinHttpOpen](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpopen).
Proxy/PAC behavior and credential-free operation still need native acceptance.

Only status 200 is accepted as an artifact. Headers are capped at 16 KiB and
the Location query buffer at 4096 UTF-16 units. The fixed 16 KiB reader allows
at most the pinned size plus one overrun-detection byte, rejects early EOF and
never returns a partial artifact. Compression is not enabled; identity encoding
is requested. Freshness/compatibility are checked before networking; exact size,
SHA-256, signature and selected identity/components are checked after transfer.
Cancellation and freshness are checked again after package validation.

## Time and cancellation boundary

The operation tracks a 180-second elapsed-time budget, with positive DNS,
connection, send and receive timeouts capped by remaining time. Cancellation is
cooperative between blocking calls and checked around body reads and validation.
This is **not** a hard wall-clock deadline or immediate cancellation guarantee:
WinHTTP, proxy discovery and OS I/O need native timeout/slow-response tests.
See [WinHttpSetTimeouts](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpsettimeouts).
Handles stay on one worker and are closed after calls return. No other thread
closes a synchronous request to cancel it; Microsoft warns about that race in
[WinHttpCloseHandle](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle).

No request URLs, signed redirect queries, response bodies or authentication data
are logged by this module. Returned errors contain only categories/codes.
`DownloadedPackage::from_bytes` applies the same pinned authentication to cache
bytes. `package_install` now performs exact store-cache lookup and whole-plan
commit. Its crate-private catalog transport reuses the source/timeout policy but
accepts a nonempty body up to 1 MiB rather than a pinned artifact length; those
bytes must immediately pass catalog verification before display or acceptance.

Tests cover hostile target forms, exact/truncated/oversized bodies, invalid read
counts, cooperative cancellation/deadline checks and the authenticated result
wrapper. They are not live WinHTTP/TLS/proxy tests. No package has been downloaded
or installed by these development tests. The original artifact transport passed
Windows compilation/Clippy in job `fd40d9b0a78b4496b76152c60868b340`. Its new
catalog branch and online integration passed Windows compilation/Clippy in job
`0c416f3ce34a41e2b18468a055220434`; native network acceptance remains pending.
