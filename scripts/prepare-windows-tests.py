#!/usr/bin/env python3
"""Cross-compile root Rust tests and collect a portable, non-release CI bundle.

No executable is run. Cargo's JSON output is authoritative: stale deps/*.exe
files are never globbed. The separate Windows script runs the exact collection.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-pc-windows-gnu"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_artifacts(messages: list[dict]) -> list[dict]:
    artifacts = {}
    finished = False
    for message in messages:
        if message.get("reason") == "build-finished":
            finished = message.get("success") is True
        if message.get("reason") != "compiler-artifact" or not message.get("profile", {}).get("test") or not message.get("executable"):
            continue
        path = Path(message["executable"])
        if path.suffix.lower() != ".exe":
            raise ValueError(f"Expected a Windows test executable: {path}")
        artifacts[str(path)] = {"name": message["target"]["name"], "source": path}
    if not finished or not artifacts:
        raise ValueError("Cargo did not report a successful build with test executables")
    names = {item["name"] for item in artifacts.values()}
    if not {"accessibility_fixture_tests", "windows_backend_tests", "coralspynext"} <= names:
        raise ValueError(f"Missing required root Windows test targets: {sorted(names)}")
    return sorted(artifacts.values(), key=lambda item: str(item["source"]))


def prepare(output: Path, cargo_json: Path | None) -> dict:
    command = ["cargo", "test", "--locked", "--all-targets", "--no-run", "--target", TARGET, "--message-format=json"]
    if cargo_json:
        raw = cargo_json.read_text(encoding="utf-8")
    else:
        result = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, check=False)
        raw = result.stdout
        if result.returncode:
            for line in raw.splitlines():
                message = json.loads(line)
                if message.get("reason") == "compiler-message":
                    print(message["message"].get("rendered", ""), file=sys.stderr)
            raise RuntimeError(f"Cargo test compilation failed ({result.returncode})")
    messages = [json.loads(line) for line in raw.splitlines() if line.strip()]
    artifacts = test_artifacts(messages)
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="coralspy-ci-tests-") as temporary:
        base = Path(temporary)
        stage = base / "verification"
        (stage / "tests").mkdir(parents=True)
        tests = []
        for artifact in artifacts:
            if artifact["source"].is_symlink():
                raise ValueError(f"Symlink is not an eligible test executable: {artifact['source']}")
            source = artifact["source"].resolve(strict=True)
            source.relative_to((ROOT / "target" / TARGET).resolve())
            destination = stage / "tests" / source.name
            if destination.exists():
                raise ValueError(f"Duplicate test executable filename: {source.name}")
            shutil.copyfile(source, destination)
            tests.append({"name": artifact["name"], "path": destination.relative_to(stage).as_posix(), "sha256": sha256(destination)})
        fixture = stage / "hook-fixtures"
        (fixture / "bin").mkdir(parents=True)
        for name in ["Test-OwnedFixture.ps1", "owned_fixture.c", "build-fixture.sh"]:
            shutil.copyfile(ROOT / "hook-engine/fixtures" / name, fixture / name)
        for arch in ["x64", "x86"]:
            name = f"owned-fixture-{arch}.exe"
            shutil.copyfile(ROOT / "hook-engine/fixtures/bin" / name, fixture / "bin" / name)
        shutil.copyfile(ROOT / "hook-engine/docs/fixture-tests.md", fixture / "README.md")
        shutil.copyfile(ROOT / "scripts/test-windows-artifact.ps1", stage / "test-windows-artifact.ps1")
        expected = []
        for source, name in [
            (ROOT / f"target/{TARGET}/release/coralspynext.exe", "coralspynext.exe"),
            (ROOT / f"hook-engine/target/{TARGET}/release/coralspy-hook-broker.exe", "coralspy-hook-broker-x64.exe"),
            (ROOT / "hook-engine/target/i686-pc-windows-gnu/release/coralspy-hook-broker.exe", "coralspy-hook-broker-x86.exe"),
        ]:
            expected.append({"path": name, "sha256": sha256(source)})
        files = [{"path": p.relative_to(stage).as_posix(), "sha256": sha256(p)} for p in sorted(stage.rglob("*")) if p.is_file()]
        manifest = {"schema": 1, "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    "target": TARGET, "test_executables": tests, "files": files,
                    "expected_release_files": expected, "application": "coralspynext.exe",
                    "hook_harness": "hook-fixtures/Test-OwnedFixture.ps1", "desktop_global_hook": False}
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        archive = output / "CoralSpyNext-ci-tests.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    z.write(path, path.relative_to(base).as_posix())
        return {"archive": str(archive), "test_executables": len(tests), "sha256": sha256(archive), "native_tests_run": False}


def self_test() -> None:
    messages = [{"reason": "compiler-artifact", "profile": {"test": True}, "executable": f"/tmp/{name}.exe", "target": {"name": name}} for name in ["accessibility_fixture_tests", "windows_backend_tests", "coralspynext"]]
    messages += [{"reason": "compiler-artifact", "profile": {"test": False}, "executable": "/tmp/not-a-test.exe", "target": {"name": "ignored"}}, {"reason": "build-finished", "success": True}]
    assert len(test_artifacts(messages)) == 3
    for invalid in [[], messages[:-1], messages + [{"reason": "build-finished", "success": False}]]:
        try:
            test_artifacts(invalid)
            raise AssertionError("incomplete or failed build accepted")
        except ValueError:
            pass
    print("prepare-windows-tests self-tests passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist/ci")
    parser.add_argument("--cargo-json", type=Path, help="Reuse a successful cargo --no-run --message-format=json log")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
    else:
        print(json.dumps(prepare(arguments.output_dir.resolve(), arguments.cargo_json), indent=2))
