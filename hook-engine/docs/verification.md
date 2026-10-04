# Verification record

Verified on 2026-10-04 in the Linux build environment. These are compilation/static/host-test results, not Windows runtime results.

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

## Not run

- Native Windows callback/injection behavior
- PowerShell parser or fixture integration execution
- UI layout/indicator visual checks on Windows
- Password-control rejection, same-integrity denial, cancellation/unhook/recovery on live Windows controls

No original program/DLL or user application was executed. The fixture harness is opt-in and creates its own visible test application; desktop-wide menu tests have additional explicit isolated-desktop switches.

## Remaining product limitations

- Menus are pre-window-procedure snapshots and may precede dynamic menu population
- Owner-data ListViews are unsupported
- Hook registration is bounded, but a target-owned blocking procedure cannot be unwound or terminated safely; delayed callbacks and temporary module cleanup are documented
- Installed broker binaries/app directory must be trusted; this is not a security boundary against malicious code running as the same user
