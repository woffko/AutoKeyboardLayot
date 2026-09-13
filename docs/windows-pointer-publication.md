# Open-reader pointer publication: native verification

The complete modular Windows core run initially passed 254/255 tests. The
open-reader store test failed with Win32 error 5 when MoveFileExW attempted to
replace CURRENT while a reader retained FILE_SHARE_READ | FILE_SHARE_DELETE.
The reader-sharing policy was intentionally kept; no test was skipped.

The isolated comparison in `tools/probe-pointer-replacement.ps1` showed:

- MoveFileExW failed, leaving both reads on the old pointer.
- ReplaceFileW and FileRenameInfoEx succeeded; the old handle read old bytes,
  while a fresh open read new bytes.

ReplaceFileW was not selected: its documented failure states can leave the
replaced path missing or renamed. The implementation uses one same-directory
FileRenameInfoEx operation with replace/POSIX flags instead, without fallback.
It bounds the UTF-16 destination, allocates a native-aligned request buffer and
checks the opened source is a non-reparse disk file. Errors after submission
are conservatively CommitUncertain. Power-loss durability remains unproven.

After the change, all **255 Windows core tests passed, none ignored**, including
the original held-reader test. All **10 selected native installer-boundary
tests** also passed. Receipts:

- `target/pointer-replacement-windows-20260912-01.json`
- `target/base-build-receipt-20260912-02.json`
- `target/windows-core-tests-20260912-02/result.json`
- `target/installer-boundary-tests-20260912-02/result.json`

These used only isolated test files/profiles, not a live installation or physical
typing. The guarded UI probe built earlier does not contain this newer fix.
No external analysis service received the failure description: its requested
job was rejected by review; diagnosis and API experiments continued locally.

Primary API contracts:

- https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information
- https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew
