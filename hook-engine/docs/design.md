# Design, protocol and review notes

## Lifecycle and trust boundaries

The GUI makes a deliberate capture request. The client checks the bounded request, resolves only the fixed sibling broker executable for x86 or x64, and places that process in a kill-on-close Job Object before sending the request. The broker accepts exactly one JSON request line on stdin, with no command-line configuration or payload-path argument. Keep stdin open while capture is active. A second `cancel\n` line, stdin EOF/error, indicator close, or deadline signals cancellation. The client allows 300 ms for graceful cancellation before terminating the one-shot broker job.

Before installing a hook, the broker creates and verifies a visible, always-on-top capture window. Selected targets are rechecked using HWND/PID/TID, limited process-query access, architecture, process protection status, account SID and integrity level. No debug privilege, elevation, arbitrary process access, remote allocation, remote thread, or remote-memory API is used. The desktop menu path additionally checks the actual callback process against the broker's security identity before capturing. Cross-bitness callbacks executed in the broker are ignored rather than sending target-pointer messages remotely.

Each broker contains only its matching internally built payload. The build checks the PE machine type. A BCrypt system RNG nonce names a new owner/SYSTEM-only directory and a new local-session mapping. Directory ACEs inherit to the extracted file. After extraction, the broker opens a read-only no-write/no-delete sharing lock, checks the exact expected length, performs a bounded read and compares all bytes to the embedded payload. `LoadLibraryExW` uses the absolute verified path and restricts dependency search to the DLL directory and System32. Same-user code able to replace the installed broker or modify this executable is outside this debugging tool's security boundary.

The callback derives the nonce from its own nonce-named module filename. It has no configure/load/write-memory/execute export. DllMain only stores its module handle. Capture callbacks use fixed-size stack buffers and the fixed mapping payload area, with no Rust heap allocation or unwind across FFI. In-progress reentrancy is ignored. The hook chain is always forwarded.

## Fixed ABI v1

All fields are fixed-width integers; no pointer, handle-sized integer or Rust enum discriminant is trusted on the wire. Both targets compile constant size/offset assertions.

- SharedMemory: 1,048,768 bytes, 8-byte aligned
- Atomic state at offset 0: Pending 0, Running 1, Complete 2
- Atomic cancelled at offset 4: 0 or 1
- RequestHeader at offset 8: 112 bytes
- ResponseHeader at offset 120: 72 bytes
- Result bytes at offset 192: at most 1 MiB

The request validates magic, exact protocol version/header/mapping lengths, a 128-bit nonce, fixed operation, architecture, all target identifiers, broker PID, monotonic creation/expiry, maximum lifetime, reserved fields and all caps. The broker keeps its private request copy. The payload copies and validates the header before claiming Pending→Running. Only the winning callback writes the response. It publishes Complete with Release; the broker observes it with Acquire and validates copied metadata before copying any bounded result bytes. Records are then parsed from that private copy.

Names are `cshook-<32 hex nonce>.dll` and `Local\\CoralSpyNext-<32 hex nonce>`. Random names isolate requests; they are correlation identifiers rather than a replacement for Windows ACLs/security identity checks.

List headers, cells, tree nodes and menu entries use a 20-byte little-endian record header followed by capped UTF-16LE text. List header records identify column index and preserve blank/Unicode/tab labels; cell records identify row/column. `captured_records` includes headers plus cells, while typed results keep `columns` separate from `cells`. Tree/menu records contain depth, item ID and typed flags. No application item-data pointer, bitmap pointer or callback pointer is read. Source-reported rows/columns/nodes use `u32::MAX` when unknown, translated to `None` in the client. RTF stays opaque raw bytes without text decoding. A truncated stream is explicitly incomplete and the client export accessor refuses it.

## Cancellation and host-process limitations

Client Job Object ownership handles parent exit. A separate broker watchdog bounds setup/input stalls to 15 seconds; the active capture deadline is at most 10 seconds. The normal path signals the mapping cancellation flag before RAII unhook. Every callback operation checks cancellation/deadline before and after target-control calls; streaming checks before copying each chunk.

These bounds apply to the broker and cooperative callback work. A target's custom/malfunctioning window procedure can block inside a same-thread control call. `UnhookWindowsHookEx` does not unwind or terminate that call. The payload keeps its own mapping view/handle, so broker cleanup does not free a still-running callback's memory. Windows can defer DLL unload until execution leaves the callback. Temporary-file removal is best-effort; process crashes or long callbacks may leave an inert nonce-named file. No registry/autostart/service hook is created.

`WH_CALLWNDPROC` observes menu initialization before the application's handler. Dynamic menus can therefore reflect pre-handler contents. This is retained to match the original hook purpose/timing and is explicitly disclosed; no completeness claim is made for a dynamically mutated menu. Owner-data ListViews are unsupported. Unsupported/protected/custom controls fail closed rather than falling back to arbitrary memory reads.

## Verification scope

Host tests exercise protocol mutations, overflow/length/cap failures, target/scope constraints, exact ABI offsets, UTF-16 records, raw RTF byte preservation and payload buffer/EDITSTREAM layout helpers. Both Windows architectures are cross-compiled and their PE imports/exports audited without running them. The owned Windows fixture and explicit opt-in PowerShell tests cover native behavior, but require a Windows runtime. Until those are executed, results must remain labeled runtime-unverified.

## Primary references

- [SetWindowsHookExW: scope and bitness](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowshookexw)
- [CallWndProc: pre-window-procedure timing](https://learn.microsoft.com/en-us/windows/win32/winmsg/callwndproc)
- [Using hooks: installation, unhook and DLL lifetime](https://learn.microsoft.com/en-us/windows/win32/winmsg/using-hooks)
- [EM_STREAMOUT and valid SF_RTF flags](https://learn.microsoft.com/en-us/windows/win32/controls/em-streamout)
- [LVM_GETITEMTEXT and owner-data limitation](https://learn.microsoft.com/en-us/windows/win32/controls/lvm-getitemtext)
- [File security and inheritance](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights)
- [IsWow64Process2: limited-query rights and Windows 10 1709 baseline](https://learn.microsoft.com/en-us/windows/win32/api/wow64apiset/nf-wow64apiset-iswow64process2)
