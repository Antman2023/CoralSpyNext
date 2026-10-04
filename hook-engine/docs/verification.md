# Verification record

Verification includes Linux build checks and the targeted owned-fixture run below. No original program or user application was executed.

## Native Windows evidence

On 2026-10-04, [Actions run 37189047036](https://github.com/Antman2023/CoralSpyNext/actions/runs/37189047036) at commit `50ccaa806f671dcec5dbed79805f55994ea9e6ae` ran the fixed helpers on Windows Server 2022:

- 19 targeted Hook cases passed for x64 and 19 for x86.
- Covered original RTF byte equality, Unicode ListView headers/cells, TreeView hierarchy, selected-thread menus, password rejection, architecture/identity/consent checks, truncation, timeout/recovery, EOF/graceful cancellation, forced broker exit, and payload unload.
- Desktop-wide tests were explicitly skipped (one per architecture).
- This entire workflow was **not green**: a separate root UIA test failed. These Hook passes do not override that release gate.
- Windows 11 interactive UI, real third-party applications, mixed DPI and global-menu behavior remain outside this evidence.

## Passed

- `cargo test --workspace`: 42 tests (31 protocol, 7 client, 4 payload helpers)
- Protocol tests include 20,000 deterministic malformed-input mutation cases, exact 32/64-bit ABI assertions, all request/result length/target/nonce/cap validators, UTF-16 record boundaries, and raw RTF preservation
- Client tests reject implicit capture/global consent, oversized/zero limits, x86 handle overflow, and extra payload-path fields; incomplete RTF cannot use the complete-export accessor
- Strict `cargo clippy --workspace --all-targets -- -D warnings` on host, x86_64-pc-windows-gnu, and i686-pc-windows-gnu
- Release builds of the two fixed payload DLLs and two embedding brokers
- PE machine types; both payloads export only `CoralSpyHook` and `DllMain`
- Static import audit rejects remote allocation/memory access/remote-thread APIs, privilege adjustment, alternate generic hook APIs, and keyboard-state APIs; only known system libraries are imported
- Owned C fixture compiles for x86/x64 with `-Wall -Wextra -Werror`
- Shell syntax and Rust formatting checks

The x86 GNU payload link reports a standard stdcall alias/fixup warning. The resulting export table contains the required undecorated `CoralSpyHook` and `DllMain`; native invocation still requires Windows tests.

## Still not established

- Windows 11 interactive UI/layout/indicator visual acceptance
- Arbitrary third-party controls, different integrity/user rejection in a live isolated session, and protected-process behavior
- Desktop-wide menu capture (not part of automated release validation)
- Full client Job Object behavior under every GUI exit/crash path

No original program/DLL or user application was executed. The fixture harness is opt-in and creates its own visible test application; desktop-wide menu tests have additional explicit isolated-desktop switches, not enabled in CI.

## Remaining product limitations

- Menus are pre-window-procedure snapshots and may precede dynamic menu population
- Owner-data ListViews are unsupported
- Hook registration is bounded, but a target-owned blocking procedure cannot be unwound or terminated safely; delayed callbacks and temporary module cleanup are documented
- Installed broker binaries/app directory must be trusted; this is not a security boundary against malicious code running as the same user
