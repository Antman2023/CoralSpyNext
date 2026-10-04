#!/usr/bin/env python3
"""Inspect our built PE files without executing them. Fail closed on forbidden APIs."""
import pathlib, re, subprocess, hashlib, json
ROOT = pathlib.Path(__file__).resolve().parent.parent
FORBIDDEN = {"CreateRemoteThread", "CreateRemoteThreadEx", "NtCreateThreadEx", "VirtualAllocEx", "VirtualProtectEx", "WriteProcessMemory", "ReadProcessMemory", "SetWindowsHookExA", "GetAsyncKeyState", "GetKeyboardState", "GetKeyState", "SetWinEventHook", "AdjustTokenPrivileges"}
ALLOWED_DLLS = {"advapi32.dll", "bcrypt.dll", "kernel32.dll", "msvcrt.dll", "ntdll.dll", "user32.dll", "gdi32.dll", "ws2_32.dll", "userenv.dll", "api-ms-win-core-synch-l1-2-0.dll"}
results=[]
for target, arch, machine in [("x86_64-pc-windows-gnu", "x64", 0x8664),("i686-pc-windows-gnu", "x86", 0x14c)]:
    tool = "x86_64-w64-mingw32-objdump" if arch == "x64" else "i686-w64-mingw32-objdump"
    for name in ["coralspy_hook_payload.dll", "coralspy-hook-broker.exe"]:
        file=ROOT/"target"/target/"release"/name
        b=file.read_bytes(); off=int.from_bytes(b[0x3c:0x40],"little")
        assert b[:2]==b"MZ" and b[off:off+4]==b"PE\0\0"
        assert int.from_bytes(b[off+4:off+6],"little")==machine, file
        text=subprocess.check_output([tool,"-p",str(file)],text=True)
        imported=set(re.findall(r"^\s*DLL Name:\s*(\S+)",text,re.M))
        # Import lines have a numeric hint and symbol; inspect words rather than
        # raw embedded broker bytes, which intentionally contain the fixed DLL.
        symbols=set(re.findall(r"^\s*[0-9a-f]+\s+\d+\s+([A-Za-z_][A-Za-z0-9_@]*)",text,re.M))
        assert not (symbols & FORBIDDEN), (file, sorted(symbols & FORBIDDEN))
        assert {s.lower() for s in imported} <= ALLOWED_DLLS, (file, imported)
        if name.endswith(".dll"):
            assert "CoralSpyHook" in text, "Missing fixed hook export"
            assert "SetWindowsHookExW" not in symbols, "Payload must not install hooks"
        results.append({"file":str(file.relative_to(ROOT)),"architecture":arch,"bytes":len(b),"sha256":hashlib.sha256(b).hexdigest(),"import_dlls":sorted(imported),"runtime_tested":False})
print(json.dumps(results,indent=2))
(ROOT/"dist").mkdir(exist_ok=True)
(ROOT/"dist"/"static-build-report.json").write_text(json.dumps(results,indent=2)+"\n")
