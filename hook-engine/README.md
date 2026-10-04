# CoralSpyNext bounded hook engine

Standalone fixed-function Windows debugging helpers. This workspace does not execute any original CoralSpy binary. Runtime activation must come from an explicit capture operation. The provided integration harness launches and targets only its own fixture.

## Public client API

`coralspy-hook-client` exposes:

- `target_architecture(Target) -> Result<Architecture, CaptureError>` using limited process-query access
- `capture(CaptureRequest, &AtomicBool) -> CaptureOutcome`, intended for a background GUI worker
- `Operation::{RichEditRtf,ListView,TreeView,MenuTarget,MenuDesktopOnce}`
- `UiLanguage::{English,SimplifiedChinese}` for the broker's mandatory indicator
- `CaptureResult { actual_target, truncated, reported_counts, captured_records, data }`
- `CaptureResult::complete_rtf_bytes()` for complete, exportable RTF only

A target consists of HWND, PID and TID, all rechecked immediately before installation. Targeted captures require same user, same integrity level, a nonprotected process, and exact x86/x64 matching. No elevation or UIPI workaround exists. Desktop menu capture is a separate operation with separate explicit consent and no selected target; it remains one-shot and bitness-specific.

Place `coralspy-hook-broker-x86.exe` and `coralspy-hook-broker-x64.exe` next to the GUI executable. No caller-supplied executable, DLL path or executable byte array is accepted. Each broker embeds the payload from its fixed internal build output. It extracts that exact byte sequence to an owner-only random temporary directory, verifies full byte equality, retains a no-write/no-delete sharing lock, and restricts dependent-library search to that directory plus System32. No loose payload needs installation.

### Bounded behavior

- Default 5 seconds; maximum 10 seconds per request
- At most 512 ListView rows × 32 columns
- At most 1,024 tree/menu records, 32 levels, 2,048 UTF-16 units per text
- At most 1 MiB output/RTF; all caps can be lowered
- Source-reported totals are separate from captured records; unavailable totals are `None`
- ListView results include bounded `LVM_GETCOLUMNW` column names plus data cells, preserving empty/Unicode/tab header text; header and cell counts stay separate
- Owner-data ListViews are explicitly unsupported
- RichEdit accepts recognized classes only; ES_PASSWORD and nonzero EM_GETPASSWORDCHAR are rejected before streaming
- Raw RTF uses documented `SF_RTF` and stays byte-exact. A truncated stream has `complete: false` and must not be exported as a complete `.rtf`
- The broker must display a close-to-cancel, always-on-top indicator before installing a hook
- Client kill-on-close Job Object, broker watchdog, automatic RAII unhook, shared cancellation and monotonic deadlines

## Build and check

Requirements: Rust 1.99, x86/x64 Windows GNU standard libraries and matching MinGW linkers. No installation or download is performed by the build script.

```sh
cargo test --locked --workspace
./scripts/build-windows.sh
```

The script builds each payload first, then embeds it in its matching broker, then statically audits both PE architectures/imports. Binaries and static reports are under `dist/`. The standalone workspace uses its own lockfile.

Windows runtime tests are intentionally opt-in; see [fixture tests](docs/fixture-tests.md). Cross-compilation is not runtime verification. Do not test against real applications or the original binaries.

## Important limitations

`WH_CALLWNDPROC` executes inside the selected target. Even bounded, fixed code can expose target-control bugs or encounter a hung/custom window procedure. Cancellation removes hook registration and signals the callback; it cannot unwind a blocked target call or safely terminate the target. An already-running callback may return later, observe cancellation/deadline, and stop before further copying. OS removal/unload and best-effort temporary-file cleanup may be delayed until callbacks return. Forced process termination may leave an inert temporary payload file; nothing starts at login or persists a hook.

Menus are observed before the target processes WM_INITMENU/WM_INITMENUPOPUP, so dynamic changes made by that handler may not be in the snapshot. Desktop one-shot capture only records a qualifying local callback of the broker's own bitness; cross-bitness broker-side callbacks are ignored. There is no password retrieval, keyboard/mouse logging, generic process-memory API, remote-thread creation, arbitrary payload loader, persistent agent, network listener, or privilege bypass.

See [design and trust boundaries](docs/design.md) for the IPC contract and review notes.
