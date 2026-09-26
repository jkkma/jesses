"""Exercise one Jesses process across two Scoop-shaped portable versions.

Run this only with a built executable whose frontend is embedded. The supplied
evidence root must not exist; it and its contents are retained after the run.
No installed Scoop app, user profile, or unrelated process is changed.
"""

from __future__ import annotations

import argparse
import csv
import ctypes
from ctypes import wintypes
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


STARTUP_TIMEOUT = 45
SECONDARY_TIMEOUT = 15
POLL_SECONDS = 0.2


class CheckFailed(RuntimeError):
    pass


def sha256(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def host_jesses_running() -> bool:
    tasklist = Path(os.environ["SystemRoot"]) / "System32/tasklist.exe"
    result = subprocess.run(
        [str(tasklist), "/FI", "IMAGENAME eq jesses.exe", "/FO", "CSV", "/NH"],
        capture_output=True,
        text=True,
        encoding="mbcs",
        errors="replace",
        check=True,
    )
    return any(
        row and row[0].casefold() == "jesses.exe"
        for row in csv.reader(io.StringIO(result.stdout))
    )


def junction(link: Path, target: Path) -> None:
    env = os.environ.copy()
    env["JESSES_TEST_LINK"] = str(link)
    env["JESSES_TEST_TARGET"] = str(target)
    command = (
        "New-Item -ItemType Junction -Path $env:JESSES_TEST_LINK "
        "-Target $env:JESSES_TEST_TARGET -ErrorAction Stop | Out-Null"
    )
    subprocess.run(
        ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command],
        env=env,
        capture_output=True,
        check=True,
    )
    if not link.is_junction() or link.resolve(strict=True) != target.resolve(strict=True):
        raise CheckFailed("Scoop-shaped directory junction did not resolve to its target")


def stage(executable: Path, root: Path) -> tuple[Path, Path, Path, Path]:
    scoop = root / "scoop"
    apps = scoop / "apps/jesses"
    persist = scoop / "persist/jesses/jesses-data"
    persist.mkdir(parents=True)
    versions = [apps / "0.1.0-test", apps / "0.1.1-test"]
    for version in versions:
        version.mkdir(parents=True)
        shutil.copy2(executable, version / "jesses.exe")
        (version / "jesses.portable").write_bytes(b"1\n")
        # The frontend is embedded. These small packaged resources are useful
        # when the supplied executable came from an assembled portable tree.
        for name in (
            "resources/runtime-contract.json",
            "resources/LICENSE",
            "resources/THIRD_PARTY_NOTICES.md",
        ):
            source = executable.parent / name
            if source.is_file() and not source.is_symlink():
                destination = version / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)
        junction(version / "jesses-data", persist)
    current = apps / "current"
    junction(current, versions[0])
    return versions[0] / "jesses.exe", current / "jesses.exe", versions[1] / "jesses.exe", persist


def child_environment(root: Path) -> dict[str, str]:
    profile = root / "isolated-profile"
    locations = {
        "USERPROFILE": profile,
        "APPDATA": profile / "AppData/Roaming",
        "LOCALAPPDATA": profile / "AppData/Local",
        "TEMP": profile / "Temp",
        "TMP": profile / "Temp",
    }
    for directory in locations.values():
        directory.mkdir(parents=True, exist_ok=True)
    blocked = {"WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "WEBVIEW2_USER_DATA_FOLDER"}
    env = {
        key: value for key, value in os.environ.items()
        if not key.upper().startswith("JESSES_") and key.upper() not in blocked
    }
    env.update({key: str(value) for key, value in locations.items()})
    env["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32")
    return env


def start(executable: Path, role: str, root: Path, env: dict[str, str], owned: list, results: list) -> subprocess.Popen:
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = subprocess.SW_HIDE
    stdout = (root / f"{role}.stdout.log").open("xb")
    stderr = (root / f"{role}.stderr.log").open("xb")
    try:
        process = subprocess.Popen(
            [str(executable)],
            cwd=executable.parent,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            startupinfo=startup,
        )
    finally:
        stdout.close()
        stderr.close()
    owned.append(process)
    results.append({"role": role, "exitCode": None, "stoppedByHarness": False})
    return process


def lock_is_held(lock: Path) -> bool:
    if not lock.is_file():
        return False
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    create_file = kernel32.CreateFileW
    create_file.argtypes = [
        wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
        wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
    ]
    create_file.restype = wintypes.HANDLE
    handle = create_file(str(lock), 0x80000000, 0x7, None, 3, 0, None)
    if handle == ctypes.c_void_p(-1).value:
        code = ctypes.get_last_error()
        if code == 32:  # ERROR_SHARING_VIOLATION
            return True
        if code == 2:  # File disappeared between is_file and CreateFileW.
            return False
        raise ctypes.WinError(code)
    close_handle = kernel32.CloseHandle
    close_handle.argtypes = [wintypes.HANDLE]
    close_handle.restype = wintypes.BOOL
    if not close_handle(handle):
        raise ctypes.WinError(ctypes.get_last_error())
    return False


def seed_history(persist: Path) -> dict:
    history = persist / "data/jobs/jobs.json"
    history.parent.mkdir(parents=True)
    job = {
        "id": "synthetic-completed-job",
        "state": "succeeded",
        "request": {
            "inputPath": str(persist / "synthetic-source.mkv"),
            "outputPath": str(persist / "synthetic-output.mkv"),
            "streamIndices": [0],
        },
        "encodeSettings": None,
        "recovery": None,
        "progressSeconds": 1.0,
        "durationSeconds": 1.0,
        "logs": ["Synthetic completed job retained across launch and restart."],
        "error": None,
        "logPath": None,
    }
    history.write_text(json.dumps({"version": 1, "jobs": [job]}) + "\n", encoding="utf-8")
    return job


def preserved_history(path: Path, expected: dict) -> bytes | None:
    try:
        content = path.read_bytes()
        record = json.loads(content)
    except (FileNotFoundError, json.JSONDecodeError):
        return None
    if record.get("version") != 1 or not isinstance(record.get("jobs"), list) or len(record["jobs"]) != 1:
        raise CheckFailed("isolated startup did not preserve synthetic job history")
    saved = record["jobs"][0]
    for field in ("id", "state", "request", "logs"):
        if saved.get(field) != expected[field]:
            raise CheckFailed(f"isolated startup changed synthetic job {field}")
    return content


def await_primary(process: subprocess.Popen, persist: Path, expected: dict, previous_write: int) -> bytes:
    history = persist / "data/jobs/jobs.json"
    lock = persist / "data/jobs/jobs.lock"
    deadline = time.monotonic() + STARTUP_TIMEOUT
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise CheckFailed("primary exited before acquiring its history lock")
        content = preserved_history(history, expected)
        # JobManager rewrites restored history before completing startup. A
        # lock alone can still expose the seed or the previous session's bytes.
        if (
            content is not None
            and history.stat().st_mtime_ns != previous_write
            and lock_is_held(lock)
        ):
            return content
        time.sleep(POLL_SECONDS)
    raise CheckFailed("primary did not preserve history and acquire its lock")


def secondary_exits(process: subprocess.Popen, primary: subprocess.Popen, history: Path, lock: Path, before: bytes) -> int:
    try:
        exit_code = process.wait(timeout=SECONDARY_TIMEOUT)
    except subprocess.TimeoutExpired as error:
        raise CheckFailed("second launch did not exit promptly") from error
    if exit_code != 0:
        raise CheckFailed("second launch did not exit successfully")
    if primary.poll() is not None:
        raise CheckFailed("primary exited during second launch")
    if not lock_is_held(lock):
        raise CheckFailed("primary no longer holds the history lock")
    if history.read_bytes() != before:
        raise CheckFailed("second launch changed saved history bytes")
    return exit_code


def stop_owned(process: subprocess.Popen, result: dict) -> None:
    if process.poll() is None:
        result["stoppedByHarness"] = True
        process.terminate()
    result["exitCode"] = process.wait(timeout=10)


def run(executable: Path, root: Path) -> int:
    if os.name != "nt":
        raise CheckFailed("this regression requires Windows")
    executable = executable.resolve(strict=True)
    if executable.name.casefold() != "jesses.exe" or not executable.is_file():
        raise CheckFailed("--executable must be a built jesses.exe")
    root = root.absolute()
    if os.path.lexists(root) or not root.parent.is_dir():
        raise CheckFailed("--root must be a new directory with an existing parent")
    if host_jesses_running():
        raise CheckFailed("a host jesses.exe is already running; leave it untouched")

    receipt = {
        "schemaVersion": 1,
        "status": "failed",
        "executableSha256": sha256(executable),
        "checks": ["no-host-jesses-process"],
        "processes": [],
    }
    root.mkdir()
    owned: list[subprocess.Popen] = []
    results: list[dict] = receipt["processes"]
    step = "stage-scoop-layout"
    try:
        first, current, second, persist = stage(executable, root)
        receipt["checks"].append(step)
        env = child_environment(root)
        history = persist / "data/jobs/jobs.json"
        lock = persist / "data/jobs/jobs.lock"
        expected = seed_history(persist)

        step = "primary-lock-and-preserved-history"
        previous_write = history.stat().st_mtime_ns
        primary = start(first, "primary", root, env, owned, results)
        before = await_primary(primary, persist, expected, previous_write)
        receipt["checks"].append(step)

        for role, launcher in (("current", current), ("other-version", second)):
            step = f"{role}-second-launch"
            child = start(launcher, role, root, env, owned, results)
            results[-1]["exitCode"] = secondary_exits(child, primary, history, lock, before)
            receipt["checks"].append(step)

        step = "lock-released-after-primary-exit"
        stop_owned(primary, results[0])
        if lock_is_held(lock):
            raise CheckFailed("history lock remained held after primary exit")
        if history.read_bytes() != before:
            raise CheckFailed("primary exit changed synthetic history bytes")
        receipt["checks"].append(step)

        step = "restart-other-version-acquires-lock"
        previous_write = history.stat().st_mtime_ns
        restarted = start(second, "restart", root, env, owned, results)
        if await_primary(restarted, persist, expected, previous_write) != before:
            raise CheckFailed("restart changed saved history bytes")
        receipt["checks"].append(step)
        receipt["historySha256"] = hashlib.sha256(before).hexdigest()
        receipt["status"] = "passed"
    except Exception as error:
        receipt["failedCheck"] = step
        receipt["failureType"] = type(error).__name__
        print(f"error during {step}: {error}", file=sys.stderr)
    finally:
        for process, result in zip(owned, results):
            try:
                stop_owned(process, result)
            except (OSError, subprocess.TimeoutExpired):
                receipt["status"] = "failed"
                receipt["failedCheck"] = "owned-process-cleanup"
        (root / "single-instance-receipt.json").write_text(
            json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
        )
    print(f"Evidence retained: {root}")
    return 0 if receipt["status"] == "passed" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    try:
        return run(args.executable, args.root)
    except (CheckFailed, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
