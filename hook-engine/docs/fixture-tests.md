# Owned Windows fixture and opt-in integration tests

## Verification status

- The standalone fixture cross-compiles successfully as **x86** and **x64** with MinGW `-Wall -Wextra -Werror`.
- The shell build script passes `bash -n`.
- **Native Windows integration tests have not been run in this Linux workspace.** No original application, uploaded executable, other user application, or Windows fixture executable was run here. Cross-compilation is not evidence of hook, UI, timeout, or cleanup behavior on Windows.
- PowerShell is not installed in this workspace; its harness has not yet been executed or parsed by PowerShell. Its expected cases are executable acceptance criteria, not recorded passes.

## Files

- `fixtures/owned_fixture.c`: independent, visible Win32 test program containing only synthetic public data.
- `fixtures/build-fixture.sh`: cross-compile both architectures when their compilers are installed; missing compilers are explicitly reported as skips.
- `fixtures/Test-OwnedFixture.ps1`: opt-in native-Windows harness, requires 64-bit PowerShell 7.
- `fixtures/bin/owned-fixture-x64.exe` and `owned-fixture-x86.exe`: generated build outputs, ignored by Git.
- `fixtures/test-results/<run>/results.json`: harness-generated result records. Raw RTF artifacts contain only the fixture's public text.

The test harness accepts a broker **directory** containing the fixed filenames `coralspy-hook-broker-x64.exe` and `coralspy-hook-broker-x86.exe`. It accepts no target PID, HWND, thread ID, fixture path, DLL path, or executable payload. It launches only the corresponding adjacent `fixtures/bin/owned-fixture-*.exe` built from this source, uses a fresh nonce and output directory, and checks the manifest PID, architecture, and window/thread ownership before addressing a fixture control. Keep the harness and binaries together in a trusted, user-owned test directory. This is a development fixture, not a sandbox for untrusted replacement binaries.

## Build the fixture

From the repository's `hook-engine/` directory with MinGW on `PATH`:

```sh
bash fixtures/build-fixture.sh
```

Equivalent individual commands, also usable in a Windows MinGW terminal:

```sh
x86_64-w64-mingw32-gcc -std=c11 -Wall -Wextra -Werror -O2 -municode -DUNICODE -D_UNICODE fixtures/owned_fixture.c -o fixtures/bin/owned-fixture-x64.exe -lcomctl32 -luser32 -lgdi32 -lshell32
i686-w64-mingw32-gcc -std=c11 -Wall -Wextra -Werror -O2 -municode -DUNICODE -D_UNICODE fixtures/owned_fixture.c -o fixtures/bin/owned-fixture-x86.exe -lcomctl32 -luser32 -lgdi32 -lshell32
```

Create `fixtures/bin` first if using the individual commands. Build matching brokers using the hook engine's separate build instructions. Both matching Rust Windows targets and MinGW compilers are required; the fixture build script never installs or modifies toolchains.

## Run targeted tests on native Windows

Use a disposable Windows 10/11 x64 test session at normal, matching user integrity. Do not run with administrative elevation. Close the fixture between manual and automated runs. The harness starts a fresh instance for each architecture and always attempts to close its own broker and fixture, including when an assertion fails.

```powershell
pwsh -NoProfile -File .\fixtures\Test-OwnedFixture.ps1 -BrokerDirectory C:\path\to\fixed-brokers -Architecture both -RunOwnedFixture
```

For a single architecture, use `-Architecture x64` or `-Architecture x86`. If a selected fixture or matching broker is missing, the harness stops before launching anything rather than silently claiming coverage. A visible fixture window and the broker's visible capture indicator are expected. Do not dismiss the indicator or interact with the fixture while an automatic case is running.

By default, the harness runs **targeted** captures only. All target HWNDs come from the newly launched fixture. It intentionally corrupts only the PID/TID fields paired with an already verified fixture HWND for target-mismatch rejection cases. It never selects a foreign window.

## Separate desktop-wide opt-in

A desktop-wide hook can touch other processes on the same desktop even when the test opens only the fixture's menu. Test that operation only inside an isolated/disposable test desktop with no user applications, private documents, browser sessions, or unrelated menus. The harness requires two additional switches:

```powershell
pwsh -NoProfile -File .\fixtures\Test-OwnedFixture.ps1 -BrokerDirectory C:\path\to\fixed-brokers -Architecture both -RunOwnedFixture -TestDesktopWideMenu -ConfirmIsolatedTestDesktop
```

The two switches record the operator's explicit choice; they do not create or verify desktop isolation. Without both, no global test is performed. Even with them, the result must name the freshly launched fixture PID. A different winner fails the test without printing its captured menu content. This case is marked `SKIP` in ordinary reports.

## Fixture content and synchronization

The visible title is `CoralSpy OWNED TEST FIXTURE - public synthetic data only`. The fixture contains:

- `RICHEDIT50W` with `Public fixture — café 日本語 😀`, a bold blue span, and two more paragraphs.
- Password-style ordinary Edit and RichEdit controls containing clearly public rejection-test placeholders, never a real password.
- A report ListView with the exact headers `Name 名`, an intentionally empty second label, and `Note` followed by a literal tab and `备注`, plus three exact Unicode rows.
- A five-node TreeView with two roots and depths `0, 1, 2, 1, 0`.
- A public application menu and popup with an enabled action, disabled item, separator, and Unicode label. A visible button activates the popup for up to five seconds.
- A static wrong-class control and a visible button for a two-second synthetic stall.

Startup takes exactly two arguments: a manifest file and an ASCII run nonce. It prints one JSON manifest to stdout and atomically refreshes that file as its state changes. The manifest includes PID, UI TID, pointer width, control HWNDs, menu handles, popup HWND while present, and `stalling`, `streaming`, and `slow_rtf` flags. No sensitive information is read from another process.

Before publishing readiness, the fixture saves its own native `EM_STREAMOUT` / `SF_RTF` output to `<manifest>.expected.rtf`. The harness compares the broker's returned bytes exactly with that independently obtained golden stream. This avoids assuming that Unicode characters always use the same ANSI code page or RTF escape spelling on every Windows locale.

The fixture exposes bounded, test-only messages on its own main window:

- `WM_APP + 41`: stall only the fixture's UI thread for a duration in `wParam`, clamped to 5,000 ms.
- `WM_APP + 42`: activate its own popup, with a five-second auto-close.
- `WM_APP + 43`: close its own menu loop.
- `WM_APP + 44`: enable/disable a fixed 2,500 ms pause inside its own RichEdit `EM_STREAMOUT` subclass, so cancellation can be tested while a callback is in flight.

The harness waits for manifest state rather than relying on a guessed startup sleep. Menu activation is repeated while the broker starts so the test does not miss a one-time activation before hook installation.

## Acceptance coverage

All cases below are currently **unrun on Windows**:

1. Raw RTF result kind, prefix, retained formatting, byte-for-byte equality with the fixture's own `SF_RTF` output, and explicit `complete` state.
2. All three exact ListView header records, including Unicode, an empty label and a literal tab; all nine cells, including accented, Greek, Japanese and supplementary-plane Unicode.
3. All five TreeView labels and their hierarchy depths.
4. Target-owned menu activation and capture of both enabled and disabled items.
5. Password RichEdit rejection as `password_control`; password Edit rejection as `password_control` or `class_mismatch`, never a successful content result.
6. Wrong control class, wrong operation class, mismatched PID/TID, and wrong architecture declaration rejection.
7. Missing visible-capture consent and out-of-range limits rejection.
8. Row/column and one-unit header-text truncation with explicit `truncated` metadata, preserving an empty header.
9. A tiny raw-RTF byte cap marked `truncated: true` and `complete: false`; partial diagnostic bytes are not exported as a document.
10. A 150 ms timeout while the fixture is deliberately unresponsive, followed by payload-module absence and successful capture after recovery.
11. Pending targeted-menu cancellation on stdin EOF.
12. Graceful in-flight cancellation via the broker's literal second stdin line `cancel\n`, a `cancelled` response, payload-module absence, and successful recapture.
13. Forced exit of only the current broker during an in-flight stream, payload-module absence after the callback finishes, and successful recapture.
14. No nonce-named `cshook-<32 hex digits>.dll` resident in the fixture after successful operations and a bounded grace period.
15. Desktop-wide one-shot menu capture only with the separate isolated-desktop opt-in.

The harness sends one exact `CaptureRequest` JSON line on stdin and expects one `CaptureOutcome` JSON object on stdout. It keeps stdin open throughout every normal capture; only explicit EOF/cancel cases close it before broker exit. Request/response types are defined in `client/src/lib.rs`; broker cancellation framing is implemented in `broker/src/windows.rs`.

### What these checks do not prove

The harness tests the fixed broker's public contract directly. It does not call the Rust client `AtomicBool` API or its kill-on-close job wrapper; those need their own native client integration tests. The forced-exit case validates the analogous broker-lifetime event, not the entire client wrapper.

Windows exposes no simple public enumeration of another process's installed hook table. The harness checks broker exit, disappearance of the per-capture payload module, and successful subsequent captures. These are observable cleanup checks, not a claim to inspect the internal OS hook table. A killed broker may leave a temporary DLL artifact because its destructors could not run; this test does not certify temporary-file cleanup after forced termination.

No test attempts a protected-process, elevated-process, cross-user, privilege-bypass, arbitrary-DLL, remote-thread, or non-fixture capture. These conditions require separate security review and appropriately isolated test design. The fixture does not automate permissions, login, or elevation.

## Record and review results

The harness stops on the first failed assertion and writes all completed cases plus that failure to `fixtures/test-results/<run>/results.json`. Setup failures may produce only an empty or partial report; an absent case is not a pass. Keep the report and public RTF sample with the Windows build/OS version used. Review cleanup behavior, indicator visibility, and error details before treating the native implementation as release-tested.

The harness keeps broker stdin open for every normal capture. It has a separate explicit stdin-EOF cancellation case, in addition to `cancel` and forced-exit cases. Closing stdin immediately after the request would cancel the test rather than exercise capture.

ListView header cases now compare exact Unicode, intentionally empty and literal-tab labels, alongside independent header/cell counts and a one-unit header-text truncation test. Header records count toward `captured_records`; data-cell assertions use `data.cells.Count`.
