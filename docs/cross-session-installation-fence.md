# Cross-session installation coordination

Window lookup and `Local\` mutexes alone cannot exclude processes in another
Windows session. The lifecycle helper now checks process snapshots before and
after same-session closure and refuses a running same-path target in another
session. Query/identity failures fail closed. Snapshot checks alone are not an
atomic startup barrier.

New agent/settings instances additionally retain a read-only handle to
`LOCALAPPDATA\AutoKeyboardLayot.installation.lock` for their entire lifetime.
It permits other readers but denies write/delete sharing. After graceful closure
and acquisition of both local instance leases, Inno opens that same file for
read/write with no sharing. A racing startup either already holds a reader and
prevents installation, or cannot obtain a reader and exits before hook/UI startup.
The installer owns the exclusive handle itself, without a separate keeper
process. It releases it only after releasing the local leases at the end of work.

The coordination file is empty, is not truncated or deleted, and lives outside
the application profile so it does not turn a clean profile into a legacy one.
Unexpected contents, reparses/non-disk files or I/O errors are not repaired
silently. Users with different LocalAppData roots use different files. Redirected
or hostile path replacement by another process with equal filesystem privileges
is not a claimed security boundary.

The Inno script compiles with `/O-`. Native unit tests exercise concurrent
readers, exclusive-install/startup conflict, release and preservation of invalid
existing contents. All 257 Windows core tests (none ignored) and 11 selected
installer-boundary tests passed in `target/windows-core-tests-20260913-01` and
`target/installer-boundary-tests-20260913-01`. These are not a live multi-session
installer acceptance test. The new combined installer still needs revalidation;
previously recorded VM acceptance used the preceding candidate.
Older binaries that do not participate in this fence still require the existing
identity checks/manual closure policy; this is not a guarantee against launching
an uncooperative old binary during an upgrade.
