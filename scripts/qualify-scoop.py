"""Qualify Jesses with an isolated copy of Scoop on Windows.

The lifecycle is deliberately split so a native interaction can occur after
``install`` and before ``finish``.  No phase removes the qualification root.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import threading
from urllib.parse import quote
import winreg
import zipfile


REPOSITORY = Path(__file__).resolve().parents[1]
PACKAGE_SPEC = importlib.util.spec_from_file_location(
    "jesses_package_desktop", REPOSITORY / "scripts/package-desktop.py"
)
if PACKAGE_SPEC is None or PACKAGE_SPEC.loader is None:
    raise RuntimeError("Unable to load the canonical desktop package verifier.")
package_desktop = importlib.util.module_from_spec(PACKAGE_SPEC)
PACKAGE_SPEC.loader.exec_module(package_desktop)
SOURCE_SCOOP = Path.home() / "scoop/apps/scoop/current"
ACCEPTANCE: Path
ROOT: Path
STATE: Path
FINAL_STATE: Path
ARCHIVE: Path
MANIFEST: Path
VERSION_A = "0.1.0-scoop-test"
VERSION_B = "0.1.1-scoop-test"
VERSION_FINAL = "0.1.2-scoop-test"
REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)


class QualificationError(RuntimeError):
    pass


def configure_root(value: Path) -> None:
    global ACCEPTANCE, ROOT, STATE, FINAL_STATE, ARCHIVE, MANIFEST
    if not value.is_absolute():
        raise QualificationError("--root must be an absolute path.")
    root = normalized(value)
    if root.name.casefold() != "scoop" or root.parent == root:
        raise QualificationError("--root must name a scoop directory inside its evidence directory.")
    ACCEPTANCE = root.parent
    ROOT = root
    STATE = root / "qualification.json"
    FINAL_STATE = root / "final-update.json"
    ARCHIVE = root / "server/jesses-portable.zip"
    MANIFEST = root / "workspace/jesses.json"


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path: Path, value: object, *, exclusive: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = "x" if exclusive else "w"
    with path.open(mode, encoding="utf-8", newline="\n") as output:
        json.dump(value, output, indent=2, sort_keys=True)
        output.write("\n")


def read_state() -> dict:
    try:
        state = json.loads(STATE.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError) as error:
        raise QualificationError(f"Missing or invalid qualification state: {STATE}") from error
    if normalized(Path(state.get("root", ""))) != ROOT:
        raise QualificationError("Qualification receipt root differs from --root.")
    return state


def normalized(path: Path) -> Path:
    return Path(os.path.abspath(path))


def require_under(path: Path, parent: Path, *, allow_parent: bool = False) -> Path:
    path = normalized(path)
    parent = normalized(parent)
    if path == parent and allow_parent:
        return path
    if parent not in path.parents:
        raise QualificationError(f"Path escapes qualification root: {path}")
    return path


def is_reparse(path: Path) -> bool:
    try:
        return bool(path.lstat().st_file_attributes & REPARSE_POINT)
    except AttributeError:
        return path.is_symlink()


def windows_root() -> Path:
    value = os.environ.get("SystemRoot") or os.environ.get("SYSTEMROOT") or os.environ.get("WINDIR")
    if not value:
        raise QualificationError("The Windows directory is unavailable in this process environment.")
    return Path(value)


def tree_summary(path: Path) -> dict:
    """Return content evidence without recording file names or contents."""
    if not path.exists() and not path.is_symlink():
        return {"exists": False}
    records: list[str] = []
    files = directories = links = total = 0

    def visit(current: Path, relative: str) -> None:
        nonlocal files, directories, links, total
        metadata = current.lstat()
        attributes = getattr(metadata, "st_file_attributes", 0)
        if attributes & REPARSE_POINT or current.is_symlink():
            links += 1
            target = str(current.resolve(strict=False)).casefold()
            records.append(f"l\0{relative}\0{target}")
            return
        if current.is_dir():
            directories += 1
            records.append(f"d\0{relative}")
            for child in sorted(current.iterdir(), key=lambda item: item.name.casefold()):
                child_relative = f"{relative}/{child.name}" if relative else child.name
                visit(child, child_relative)
            return
        if current.is_file():
            files += 1
            total += metadata.st_size
            records.append(f"f\0{relative}\0{metadata.st_size}\0{digest(current)}")
            return
        records.append(f"o\0{relative}\0{metadata.st_mode}")

    visit(path, "")
    value = hashlib.sha256("\n".join(records).encode()).hexdigest()
    return {"exists": True, "sha256": value, "files": files, "directories": directories, "links": links, "bytes": total}


def registry_value(root: int, key: str, name: str) -> dict:
    try:
        with winreg.OpenKey(root, key) as handle:
            value, kind = winreg.QueryValueEx(handle, name)
    except FileNotFoundError:
        return {"exists": False}
    encoded = str(value).encode("utf-8", "surrogatepass")
    return {"exists": True, "kind": kind, "sha256": hashlib.sha256(encoded).hexdigest()}


def host_snapshot() -> dict:
    user_profile = Path(os.environ["USERPROFILE"])
    appdata = Path(os.environ["APPDATA"])
    local_appdata = Path(os.environ["LOCALAPPDATA"])
    program_data = Path(os.environ["ProgramData"])
    host_scoop = Path(os.environ.get("SCOOP", user_profile / "scoop"))
    xdg = Path(os.environ.get("XDG_CONFIG_HOME", user_profile / ".config"))
    protected = {
        "roaming_profile": appdata / "io.github.jkkma.jesses",
        "local_profile": local_appdata / "io.github.jkkma.jesses",
        "host_scoop_app": host_scoop / "apps/jesses",
        "host_scoop_persist": host_scoop / "persist/jesses",
        "host_scoop_shim_exe": host_scoop / "shims/jesses.exe",
        "host_scoop_shim_metadata": host_scoop / "shims/jesses.shim",
        "host_scoop_buckets": host_scoop / "buckets",
        "host_scoop_config": host_scoop / "config.json",
        "host_xdg_config": xdg / "scoop/config.json",
        "user_start_menu": appdata / "Microsoft/Windows/Start Menu/Programs/Jesses.lnk",
        "system_start_menu": program_data / "Microsoft/Windows/Start Menu/Programs/Jesses.lnk",
    }
    return {
        "paths": {name: tree_summary(path) for name, path in protected.items()},
        "user_path": registry_value(winreg.HKEY_CURRENT_USER, "Environment", "Path"),
        "system_path": registry_value(
            winreg.HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            "Path",
        ),
    }


def assert_host_unchanged(state: dict) -> None:
    current = host_snapshot()
    if current != state["host_before"]:
        raise QualificationError("Host Scoop/config/PATH/Jesses data snapshot changed; qualification stopped.")


def validate_archive(path: Path) -> None:
    try:
        package_desktop.verify_portable_archive(path)
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        raise QualificationError(f"Invalid portable candidate: {error}") from error


def assert_scoop_adapter_unchanged(state: dict) -> None:
    system_script = ROOT / "apps/scoop/current/lib/system.ps1"
    expected = state.get("isolated_system_sha256")
    if not isinstance(expected, str) or is_reparse(system_script) or not system_script.is_file():
        raise QualificationError("The isolated Scoop environment adapter is missing or redirected.")
    if digest(system_script) != expected:
        raise QualificationError("The isolated Scoop environment adapter changed after preparation.")


PROCESS_ENV_ADAPTER = r'''

# Jesses isolated Scoop qualification: keep Scoop environment changes in this
# child process. The unmodified source hash is recorded by qualify-scoop.py.
if ($env:JESSES_SCOOP_QUALIFICATION -ne '1') {
    throw 'This isolated Scoop copy may run only through qualify-scoop.py.'
}
function Publish-EnvVar { }
function Get-EnvVar {
    param([string]$Name, [switch]$Global)
    $scope = if ($Global) { 'Machine' } else { 'Process' }
    [Environment]::GetEnvironmentVariable($Name, $scope)
}
function Set-EnvVar {
    param([string]$Name, [string]$Value, [switch]$Global)
    if ($Global) { throw 'Global environment writes are disabled for qualification.' }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
}
'''


def prepare(archive: Path, source_scoop: Path) -> None:
    if os.name != "nt":
        raise QualificationError("Scoop qualification requires Windows.")
    if ROOT.exists():
        raise QualificationError(f"Qualification root already exists; it is preserved: {ROOT}")
    validate_archive(archive)
    source_scoop = source_scoop.resolve(strict=True)
    if not (source_scoop / "bin/scoop.ps1").is_file() or not (source_scoop / "lib/system.ps1").is_file():
        raise QualificationError(f"Invalid Scoop source: {source_scoop}")

    before = host_snapshot()
    (ROOT / "apps/scoop").mkdir(parents=True)
    copied = ROOT / "apps/scoop/current"
    shutil.copytree(source_scoop, copied, symlinks=False)
    ARCHIVE.parent.mkdir(parents=True)
    shutil.copyfile(archive, ARCHIVE)
    if digest(archive) != digest(ARCHIVE):
        raise QualificationError("Candidate copy hash differs from its source.")

    system_script = copied / "lib/system.ps1"
    original_system_hash = digest(system_script)
    with system_script.open("a", encoding="utf-8", newline="\n") as output:
        output.write(PROCESS_ENV_ADAPTER)
    (ROOT / "cache").mkdir()
    (ROOT / "buckets").mkdir()
    (ROOT / "global").mkdir()
    (ROOT / "shims").mkdir()
    (ROOT / "temp").mkdir()
    (ROOT / "xdg/scoop").mkdir(parents=True)
    config = {
        "last_update": "2099-01-01T00:00:00.0000000+00:00",
        "aria2-enabled": False,
        "use_external_7zip": False,
        "use_isolated_path": True,
    }
    write_json(ROOT / "config.json", config, exclusive=True)
    write_json(ROOT / "xdg/scoop/config.json", config, exclusive=True)
    state = {
        "schema": 1,
        "phase": "prepared",
        "root": str(ROOT),
        "candidate_sha256": digest(ARCHIVE),
        "candidate_bytes": ARCHIVE.stat().st_size,
        "source_scoop": str(source_scoop),
        "source_system_sha256": original_system_hash,
        "isolated_system_sha256": digest(system_script),
        "adapter_boundary": (
            "Scoop filesystem, manifest hash, shim, junction, persistence, update, reset and uninstall behavior are qualified. "
            "The copied Scoop adapter confines environment writes to the child process, so this run does not qualify user PATH registration."
        ),
        "host_before": before,
        "commands": [],
    }
    write_json(STATE, state, exclusive=True)
    assert_host_unchanged(state)
    print(f"Prepared isolated Scoop root: {ROOT}")


def child_environment() -> dict[str, str]:
    root = require_under(ROOT, ACCEPTANCE)
    env = os.environ.copy()
    system_root = windows_root()
    windows_modules = system_root / "System32/WindowsPowerShell/v1.0/Modules"
    inherited_modules = env.get("PSModulePath") or env.get("PSMODULEPATH") or ""
    env.update(
        {
            "SCOOP": str(root),
            "SCOOP_GLOBAL": str(root / "global"),
            "SCOOP_CACHE": str(root / "cache"),
            "SCOOP_PATH": str(root / "shims"),
            "XDG_CONFIG_HOME": str(root / "xdg"),
            "TEMP": str(root / "temp"),
            "TMP": str(root / "temp"),
            "JESSES_SCOOP_QUALIFICATION": "1",
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
            "PATH": ";".join(
                [
                    str(root / "shims"),
                    str(system_root / "System32"),
                    str(system_root),
                    str(system_root / "System32/WindowsPowerShell/v1.0"),
                ]
            ),
            "PSModulePath": ";".join(part for part in [str(windows_modules), inherited_modules] if part),
        }
    )
    return env


def validate_isolation_tree() -> None:
    root = require_under(ROOT, ACCEPTANCE)
    for configured in [root, root / "global", root / "cache", root / "xdg", root / "temp"]:
        require_under(configured, root, allow_parent=True)
        if is_reparse(configured):
            raise QualificationError(f"Configured Scoop directory is redirected: {configured}")
    for current, directories, files in os.walk(root, followlinks=False):
        for name in directories + files:
            item = Path(current) / name
            require_under(item, root)
            if is_reparse(item):
                resolved = item.resolve(strict=True)
                require_under(resolved, root, allow_parent=True)


def powershell() -> Path:
    candidate = windows_root() / "System32/WindowsPowerShell/v1.0/powershell.exe"
    if not candidate.is_file():
        raise QualificationError(f"Windows PowerShell is unavailable: {candidate}")
    return candidate


def run_scoop(state: dict, *arguments: str, receipt: Path = None, log_prefix: str = "scoop") -> None:
    receipt = STATE if receipt is None else receipt
    validate_isolation_tree()
    assert_scoop_adapter_unchanged(state)
    assert_host_unchanged(state)
    command = [
        str(powershell()),
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        str(ROOT / "apps/scoop/current/bin/scoop.ps1"),
        *arguments,
    ]
    try:
        completed = subprocess.run(
            command,
            env=child_environment(),
            cwd=ROOT,
            text=True,
            capture_output=True,
            timeout=1200,
        )
    except subprocess.TimeoutExpired as error:
        raise QualificationError(f"Scoop command timed out after 1200 seconds: {' '.join(arguments)}") from error
    log_number = len(state["commands"]) + 1
    log_path = ROOT / f"evidence/{log_prefix}-{log_number:02d}-{'-'.join(arguments[:2])}.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text(completed.stdout + completed.stderr, encoding="utf-8")
    state["commands"].append({"arguments": list(arguments), "exit_code": completed.returncode, "log": str(log_path)})
    write_json(receipt, state)
    assert_scoop_adapter_unchanged(state)
    assert_host_unchanged(state)
    if completed.returncode:
        raise QualificationError(f"Scoop command failed ({completed.returncode}); see {log_path}")


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@contextlib.contextmanager
def archive_server(archive: Path = None):
    archive = ARCHIVE if archive is None else archive
    handler = lambda *args, **kwargs: QuietHandler(*args, directory=str(archive.parent), **kwargs)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/{quote(archive.name)}"
    finally:
        server.shutdown()
        worker.join(timeout=10)
        server.server_close()


def write_manifest(version: str, url: str, archive: Path = None) -> None:
    archive = ARCHIVE if archive is None else archive
    value = {
        "version": version,
        "description": "Jesses isolated Scoop qualification candidate",
        "homepage": "https://github.com/jkkma/jesses",
        "license": "GPL-3.0-only",
        "architecture": {"64bit": {"url": url, "hash": digest(archive)}},
        "bin": "jesses.exe",
        "persist": "jesses-data",
    }
    if set(value) & {"shortcuts", "env_add_path", "installer", "uninstaller", "pre_install", "post_install"}:
        raise QualificationError("Qualification manifest contains a forbidden integration hook.")
    write_json(MANIFEST, value)


def assert_install(version: str) -> Path:
    version_dir = ROOT / f"apps/jesses/{version}"
    executable = version_dir / "jesses.exe"
    current = ROOT / "apps/jesses/current"
    data_link = version_dir / "jesses-data"
    persist = ROOT / "persist/jesses/jesses-data"
    for path in [version_dir, executable, current, data_link, persist, ROOT / "shims/jesses.exe", ROOT / "shims/jesses.shim"]:
        if not path.exists():
            raise QualificationError(f"Expected Scoop installation item is missing: {path}")
    if not is_reparse(current) or current.resolve() != version_dir.resolve():
        raise QualificationError("Scoop current junction does not select the expected test version.")
    if not is_reparse(data_link) or data_link.resolve() != persist.resolve():
        raise QualificationError("jesses-data is not linked to isolated Scoop persistence.")
    if is_reparse(persist) or not persist.is_dir():
        raise QualificationError("Scoop persistence target is not an ordinary directory.")
    shim = (ROOT / "shims/jesses.shim").read_text(encoding="utf-8-sig")
    if str(ROOT).casefold() not in shim.casefold():
        raise QualificationError("Jesses shim does not point inside the isolated Scoop root.")
    return executable


def install() -> None:
    state = read_state()
    if state["phase"] != "prepared" or digest(ARCHIVE) != state["candidate_sha256"]:
        raise QualificationError("Install requires the unchanged prepared candidate.")
    assert_scoop_adapter_unchanged(state)
    (ROOT / "buckets").mkdir(exist_ok=True)
    with archive_server() as url:
        write_manifest(VERSION_A, url)
        run_scoop(state, "install", "-u", "-i", str(MANIFEST))
    executable = assert_install(VERSION_A)
    state["phase"] = "installed"
    state["app_path"] = str(executable)
    write_json(STATE, state)
    assert_host_unchanged(state)
    print(f"Installed native-interaction candidate: {executable}")
    print("Close Jesses after native interaction, then run the finish phase.")


def assert_persist_unchanged(expected: dict) -> None:
    actual = tree_summary(ROOT / "persist/jesses/jesses-data")
    if actual != expected:
        raise QualificationError("Persisted Jesses data changed during the Scoop lifecycle.")


def assert_uninstalled() -> None:
    expected_absent = [ROOT / "apps/jesses", ROOT / "shims/jesses.exe", ROOT / "shims/jesses.shim"]
    leftovers = [str(path) for path in expected_absent if os.path.lexists(path)]
    if leftovers:
        raise QualificationError("Scoop uninstall left isolated app or shim files behind: " + ", ".join(leftovers))


def finish() -> None:
    state = read_state()
    if state["phase"] != "installed":
        raise QualificationError("Finish requires a completed install/native-interaction phase.")
    assert_scoop_adapter_unchanged(state)
    assert_install(VERSION_A)
    persist = ROOT / "persist/jesses/jesses-data"
    required = [persist / item for item in ["config", "data", "cache", "logs", "cache/webview", "data/jobs/jobs.json"]]
    missing = [str(path) for path in required if not path.exists()]
    if missing:
        raise QualificationError("Native interaction did not create required portable state: " + ", ".join(missing))
    before = tree_summary(persist)
    state["persist_before_lifecycle"] = before
    write_json(STATE, state)

    with archive_server() as url:
        write_manifest(VERSION_B, url)
        run_scoop(state, "update", "-i", "jesses")
    assert_install(VERSION_B)
    if not (ROOT / f"apps/jesses/{VERSION_A}").is_dir():
        raise QualificationError("The explicitly labeled original version directory was not retained for inspection.")
    assert_persist_unchanged(before)

    run_scoop(state, "reset", "jesses")
    assert_install(VERSION_B)
    assert_persist_unchanged(before)

    run_scoop(state, "uninstall", "jesses")
    assert_uninstalled()
    assert_persist_unchanged(before)

    with archive_server() as url:
        write_manifest(VERSION_B, url)
        run_scoop(state, "install", "-u", "-i", str(MANIFEST))
    executable = assert_install(VERSION_B)
    assert_persist_unchanged(before)
    state["phase"] = "complete"
    state["final_app_path"] = str(executable)
    state["persist_after_lifecycle"] = tree_summary(persist)
    write_json(STATE, state)
    assert_host_unchanged(state)
    print(f"Scoop lifecycle qualification passed; reinstalled app: {executable}")
    print(f"Receipt: {STATE}")


def final_update(archive: Path) -> None:
    state = read_state()
    if state["phase"] != "complete":
        raise QualificationError("Final update requires the completed original-candidate lifecycle.")
    assert_scoop_adapter_unchanged(state)
    if FINAL_STATE.exists():
        raise QualificationError(f"Final update receipt already exists and is preserved: {FINAL_STATE}")
    original_receipt_hash = digest(STATE)
    validate_archive(archive)
    final_archive = ROOT / "server/jesses-portable-final.zip"
    if final_archive.exists():
        raise QualificationError(f"Final candidate copy already exists and is preserved: {final_archive}")
    shutil.copyfile(archive, final_archive)
    if digest(archive) != digest(final_archive):
        raise QualificationError("Final candidate copy hash differs from its source.")
    before = tree_summary(ROOT / "persist/jesses/jesses-data")
    if before != state["persist_after_lifecycle"]:
        raise QualificationError("Persisted Jesses data changed after lifecycle qualification.")
    final_state = {
        "schema": 1,
        "phase": "updating",
        "root": str(ROOT),
        "original_qualification": str(STATE),
        "original_qualification_sha256": original_receipt_hash,
        "original_candidate_sha256": state["candidate_sha256"],
        "final_candidate_source": str(archive.resolve(strict=True)),
        "final_candidate_sha256": digest(final_archive),
        "final_candidate_bytes": final_archive.stat().st_size,
        "persist_before_final_update": before,
        "host_before": state["host_before"],
        "isolated_system_sha256": state["isolated_system_sha256"],
        "commands": [],
    }
    write_json(FINAL_STATE, final_state, exclusive=True)
    with archive_server(final_archive) as url:
        write_manifest(VERSION_FINAL, url, final_archive)
        run_scoop(
            final_state,
            "update",
            "-i",
            "jesses",
            receipt=FINAL_STATE,
            log_prefix="final-scoop",
        )
    executable = assert_install(VERSION_FINAL)
    assert_persist_unchanged(before)
    if digest(STATE) != original_receipt_hash:
        raise QualificationError("Original lifecycle qualification receipt was modified.")
    final_state["phase"] = "final-installed"
    final_state["final_candidate_app_path"] = str(executable)
    final_state["persist_after_final_update"] = tree_summary(ROOT / "persist/jesses/jesses-data")
    write_json(FINAL_STATE, final_state)
    assert_host_unchanged(final_state)
    print(f"Installed final native-restart candidate: {executable}")
    print(f"Receipt: {FINAL_STATE}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = subcommands.add_parser("prepare")
    prepare_parser.add_argument("--root", type=Path, required=True)
    prepare_parser.add_argument("--archive", type=Path, required=True)
    prepare_parser.add_argument("--source-scoop", type=Path, default=SOURCE_SCOOP)
    install_parser = subcommands.add_parser("install")
    install_parser.add_argument("--root", type=Path, required=True)
    finish_parser = subcommands.add_parser("finish")
    finish_parser.add_argument("--root", type=Path, required=True)
    final_parser = subcommands.add_parser("final-update")
    final_parser.add_argument("--root", type=Path, required=True)
    final_parser.add_argument("--archive", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    arguments = parse_args()
    try:
        configure_root(arguments.root)
        if arguments.command == "prepare":
            prepare(arguments.archive, arguments.source_scoop)
        elif arguments.command == "install":
            install()
        elif arguments.command == "finish":
            finish()
        else:
            final_update(arguments.archive)
    except (QualificationError, OSError, KeyError, zipfile.BadZipFile) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
