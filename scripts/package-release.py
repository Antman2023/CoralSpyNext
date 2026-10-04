#!/usr/bin/env python3
"""Package existing source-built GNU binaries; never build or execute an EXE.

Python 3.11+, Git, and cached Cargo dependencies are required. Run after building
the GUI, both fixed brokers and both owned fixtures. Native verification is a
separate, mandatory release-workflow gate, not a claim made by this packager.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import struct
import subprocess
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-pc-windows-gnu"
SOURCE_DIRS = {"src", "tests", "assets", "scripts", "ci", "docs", "hook-engine", "third-party-licenses", ".github"}
SOURCE_FILES = {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "build.rs", "build_stamp.rs", "README.md", "TESTING.md", "THIRD_PARTY.md", "LICENSE", ".gitignore"}
SOURCE_SUFFIXES = {".rs", ".toml", ".lock", ".md", ".txt", ".json", ".py", ".ps1", ".sh", ".c", ".h", ".yml", ".yaml", ".rc", ".manifest", ".ico", ".png", ".svg"}


def run(*args: str, cwd: Path = ROOT) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def pe_machine(path: Path, expected: int) -> None:
    data = path.read_bytes()
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError(f"Not a PE executable: {path}")
    offset = struct.unpack_from("<I", data, 60)[0]
    if offset + 6 > len(data) or data[offset:offset + 4] != b"PE\0\0":
        raise ValueError(f"Invalid PE header: {path}")
    if struct.unpack_from("<H", data, offset + 4)[0] != expected:
        raise ValueError(f"Wrong PE architecture: {path}")


def validate_tag(version: str, ref: str) -> None:
    if ref.startswith("refs/tags/") and ref != f"refs/tags/v{version}":
        raise ValueError(f"Release tag {ref!r} must equal v{version} from Cargo.toml")


def source_allowed(name: str) -> bool:
    p = PurePosixPath(name)
    if p.is_absolute() or ".." in p.parts:
        return False
    if any(part in {"target", "dist", "bin", ".git", "original", "originals", "node_modules"} for part in p.parts):
        return False
    if any(part.startswith(".") and part not in {".github", ".gitignore"} for part in p.parts):
        return False
    return name in SOURCE_FILES or (p.parts[0] in SOURCE_DIRS and (p.suffix.lower() in SOURCE_SUFFIXES or p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")) or p.name.upper().endswith("-LICENSE") or p.name == ".gitignore"))


def copy_file(source: Path, destination: Path) -> None:
    if not source.is_file() or source.is_symlink():
        raise ValueError(f"Expected a regular, non-symlink file: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def dependency_licenses(destination: Path, tracked: list[str]) -> dict:
    packages = {}
    for manifest, targets in [(ROOT / "Cargo.toml", [TARGET]),
                              (ROOT / "hook-engine/Cargo.toml", [TARGET, "i686-pc-windows-gnu"])]:
        for target in targets:
            metadata = json.loads(run("cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target, "--manifest-path", str(manifest)))
            for package in metadata["packages"]:
                if package.get("source"):
                    packages[package["id"]] = package
    inventory = []
    for package in sorted(packages.values(), key=lambda p: (p["name"], p["version"])):
        base = Path(package["manifest_path"]).parent.resolve()
        key = f'{package["name"]}-{package["version"]}'
        candidates = {p for p in base.rglob("*") if p.is_file() and len(p.relative_to(base).parts) <= 3 and p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT"))}
        if package.get("license_file"):
            candidates.add((base / package["license_file"]).resolve())
        files = []
        for source in sorted(candidates):
            relative = source.resolve().relative_to(base)
            target = destination / key / relative
            copy_file(source, target)
            files.append(target.relative_to(destination).as_posix())
        inventory.append({"name": package["name"], "version": package["version"], "license": package.get("license"), "repository": package.get("repository"), "license_files": files, "cached_license_text_found": bool(files)})
    overlay = []
    for name in tracked:
        if name.startswith("third-party-licenses/") and source_allowed(name):
            relative = Path(name).relative_to("third-party-licenses")
            copy_file(ROOT / name, destination / "upstream" / relative)
            overlay.append((Path("upstream") / relative).as_posix())
    report = {"dependencies": inventory, "upstream_supplement_files": overlay,
              "note": "License expressions come from locked cached Cargo package metadata. Upstream supplemental notices are included separately; no missing license text is invented."}
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "DEPENDENCIES.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return {"dependency_count": len(inventory), "cached_text_missing": [f'{p["name"]}-{p["version"]}' for p in inventory if not p["cached_license_text_found"]], "supplement_files": overlay}


def package(output: Path) -> dict:
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    validate_tag(version, os.environ.get("GITHUB_REF", ""))
    commit = run("git", "rev-parse", "HEAD").strip()
    tracked = run("git", "ls-files", "-z").split("\0")
    tracked = [name for name in tracked if name]
    binaries = [
        (ROOT / f"target/{TARGET}/release/coralspynext.exe", "coralspynext.exe", 0x8664),
        (ROOT / f"hook-engine/target/{TARGET}/release/coralspy-hook-broker.exe", "coralspy-hook-broker-x64.exe", 0x8664),
        (ROOT / "hook-engine/target/i686-pc-windows-gnu/release/coralspy-hook-broker.exe", "coralspy-hook-broker-x86.exe", 0x014C),
    ]
    folder = f"CoralSpyNext-v{version}-windows-x64"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="coralspy-package-") as temporary:
        stage = Path(temporary) / folder
        stage.mkdir()
        for source, name, machine in binaries:
            pe_machine(source, machine)
            copy_file(source, stage / name)
        for name in tracked:
            if source_allowed(name):
                copy_file(ROOT / name, stage / "source" / name)
        for name in ["README.md", "TESTING.md", "THIRD_PARTY.md", "LICENSE"]:
            copy_file(ROOT / name, stage / name)
        copy_file(ROOT / "hook-engine/README.md", stage / "hook-engine/README.md")
        for directory in ["docs", "assets", "hook-engine/docs"]:
            for name in tracked:
                if name.startswith(directory + "/") and source_allowed(name):
                    copy_file(ROOT / name, stage / name)
        fixture = stage / "tests/hook-fixtures"
        for name in ["Test-OwnedFixture.ps1", "owned_fixture.c", "build-fixture.sh"]:
            copy_file(ROOT / "hook-engine/fixtures" / name, fixture / name)
        for architecture, machine in [("x64", 0x8664), ("x86", 0x014C)]:
            source = ROOT / f"hook-engine/fixtures/bin/owned-fixture-{architecture}.exe"
            pe_machine(source, machine)
            copy_file(source, fixture / "bin" / source.name)
        license_summary = dependency_licenses(stage / "licenses", tracked)
        info = {"project": "CoralSpyNext", "version": version, "commit": commit,
                "tracked_worktree_dirty": bool(run("git", "status", "--porcelain", "--untracked-files=no").strip()),
                "target": TARGET, "broker_targets": [TARGET, "i686-pc-windows-gnu"],
                "rust": run("rustc", "-Vv").strip(), "source_date_epoch": os.environ.get("SOURCE_DATE_EPOCH"),
                "workflow_run": os.environ.get("GITHUB_RUN_ID"),
                "validation_at_packaging": {"windows_native_tests": "not run by packager", "hook_fixture_tests": "not run by packager", "desktop_global_hook": "not enabled"},
                "release_gate": "Publish only after the separate Windows verification job passes for this exact artifact.",
                "licenses": license_summary,
                "executables": [{"file": name, "sha256": digest(stage / name)} for _, name, _ in binaries]}
        (stage / "BUILD-INFO.json").write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
        files = sorted(p for p in stage.rglob("*") if p.is_file())
        (stage / "SHA256.txt").write_text("".join(f"{digest(p)}  {p.relative_to(stage).as_posix()}\n" for p in files), encoding="utf-8")
        archive = output / f"{folder}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            for path in sorted(p for p in stage.rglob("*") if p.is_file()):
                z.write(path, path.relative_to(Path(temporary)).as_posix())
        result = {"version": version, "commit": commit, "package_root": folder, "archive": archive.name, "sha256": digest(archive)}
        (output / "release-manifest.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        (output / f"{folder}.zip.sha256").write_text(f'{result["sha256"]}  {archive.name}\n', encoding="utf-8")
        return result


def self_test() -> None:
    validate_tag("1.2.3", "refs/tags/v1.2.3")
    validate_tag("1.2.3", "refs/heads/main")
    try:
        validate_tag("1.2.3", "refs/tags/v1.2.4")
        raise AssertionError("mismatched release tag accepted")
    except ValueError:
        pass
    for name in ["src/main.rs", "Cargo.lock", "assets/app.ico", ".github/workflows/release.yml", "third-party-licenses/egui/LICENSE-MIT", "third-party-licenses/khronos_api-3.1.0/ANGLE-LICENSE"]:
        assert source_allowed(name), name
    for name in [".env", "../secret.rs", "/tmp/file.rs", "hook-engine/target/debug/build.rs", "original/CoralSpy.exe", "src/secret.key", "src/.credentials.json"]:
        assert not source_allowed(name), name
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / "fixture.exe"
        data = bytearray(80)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 64)
        data[64:68] = b"PE\0\0"
        struct.pack_into("<H", data, 68, 0x8664)
        path.write_bytes(data)
        pe_machine(path, 0x8664)
        try:
            pe_machine(path, 0x014C)
            raise AssertionError("wrong architecture accepted")
        except ValueError:
            pass
    print("package-release self-tests passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist/release")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
    else:
        print(json.dumps(package(arguments.output_dir.resolve()), indent=2))
