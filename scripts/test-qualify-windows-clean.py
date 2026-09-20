"""Focused safety tests for qualify-windows-clean.ps1."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "scripts" / "qualify-windows-clean.ps1"


@unittest.skipUnless(sys.platform == "win32", "Windows PowerShell qualification tests")
class WindowsCleanQualificationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        version = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        if not version.startswith("5.1"):
            raise unittest.SkipTest(f"Windows PowerShell 5.1 is unavailable: {version}")

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-windows-clean-test-")
        self.root = Path(self.temporary.name)
        self.probe = self.root / "package_tools.exe"
        self.probe.write_bytes(b"MZ test probe; malformed archives fail before execution\n")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def record(path: str, payload: bytes) -> dict[str, object]:
        return {"path": path, "size": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}

    def base_files(self) -> dict[str, bytes]:
        return {
            "jesses.exe": b"MZ synthetic application\n",
            "jesses.portable": b"1\n",
            "resources/runtime-contract.json": b"{}\n",
            "resources/tools/manifest.json": b"{}\n",
        }

    def write_archive(
        self,
        name: str,
        files: dict[str, bytes],
        manifest_files: dict[str, bytes] | None = None,
        special_entries: list[tuple[zipfile.ZipInfo, bytes]] | None = None,
    ) -> Path:
        archive = self.root / name
        listed = files if manifest_files is None else manifest_files
        manifest = {
            "schemaVersion": 1,
            "product": "jesses",
            "version": "test",
            "target": "x86_64-pc-windows-msvc",
            "profile": "release",
            "storage": "portable",
            "files": [self.record(path, payload) for path, payload in listed.items()],
        }
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path, payload in files.items():
                bundle.writestr(path, payload)
            for entry, payload in special_entries or []:
                bundle.writestr(entry, payload)
            bundle.writestr("package-manifest.json", json.dumps(manifest, sort_keys=True))
        return archive

    def run_failure(self, archive: Path, evidence_name: str = "evidence with spaces") -> tuple[subprocess.CompletedProcess[str], Path, dict]:
        evidence = self.root / evidence_name
        result = subprocess.run(
            [
                "powershell.exe",
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(RUNNER),
                "-Scope",
                "RestrictedHost",
                "-Archive",
                str(archive),
                "-Probe",
                str(self.probe),
                "-EvidenceDirectory",
                str(evidence),
            ],
            capture_output=True,
            text=True,
            errors="replace",
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt_path = evidence / "qualification.json"
        self.assertTrue(receipt_path.is_file(), result.stdout + result.stderr)
        receipt = json.loads(receipt_path.read_text(encoding="utf-8-sig"))
        self.assertEqual(receipt["status"], "failed")
        self.assertFalse(receipt["qualifiesFreshImage"])
        return result, evidence, receipt

    def test_traversal_entry_is_rejected_without_escape(self) -> None:
        files = self.base_files()
        files["../escaped.txt"] = b"must not escape\n"
        archive = self.write_archive("traversal.zip", files)
        _, evidence, receipt = self.run_failure(archive)
        self.assertIn("Unsafe", receipt["error"])
        self.assertFalse((evidence.parent / "escaped.txt").exists())

    def test_absolute_entry_is_rejected(self) -> None:
        files = self.base_files()
        files["C:/escaped.txt"] = b"must not escape\n"
        archive = self.write_archive("absolute.zip", files)
        _, _, receipt = self.run_failure(archive)
        self.assertIn("Unsafe", receipt["error"])

    def test_case_colliding_entries_are_rejected(self) -> None:
        files = self.base_files()
        files["JESSES.EXE"] = files["jesses.exe"]
        archive = self.write_archive("case-collision.zip", files)
        _, _, receipt = self.run_failure(archive)
        self.assertIn("case-colliding", receipt["error"])

    def test_reparse_entry_is_rejected(self) -> None:
        entry = zipfile.ZipInfo("resources/tools/link")
        entry.create_system = 3
        entry.external_attr = (stat.S_IFLNK | 0o777) << 16
        archive = self.write_archive("reparse.zip", self.base_files(), special_entries=[(entry, b"target")])
        _, _, receipt = self.run_failure(archive)
        self.assertIn("links are forbidden", receipt["error"])

    def test_missing_manifest_payload_is_rejected(self) -> None:
        files = self.base_files()
        listed = dict(files)
        listed["resources/missing.bin"] = b"listed but absent\n"
        archive = self.write_archive("missing.zip", files, listed)
        _, _, receipt = self.run_failure(archive)
        self.assertIn("differ from package-manifest.json", receipt["error"])

    def test_unlisted_payload_is_rejected(self) -> None:
        files = self.base_files()
        listed = dict(files)
        files["resources/unlisted.bin"] = b"present but unlisted\n"
        archive = self.write_archive("extra.zip", files, listed)
        _, _, receipt = self.run_failure(archive)
        self.assertIn("differ from package-manifest.json", receipt["error"])

    def test_existing_evidence_directory_is_not_modified(self) -> None:
        archive = self.write_archive("existing-evidence.zip", self.base_files())
        evidence = self.root / "existing"
        evidence.mkdir()
        marker = evidence / "keep.txt"
        marker.write_text("unchanged\n", encoding="utf-8")
        result = subprocess.run(
            [
                "powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(RUNNER),
                "-Scope", "RestrictedHost", "-Archive", str(archive), "-Probe", str(self.probe),
                "-EvidenceDirectory", str(evidence),
            ],
            capture_output=True, text=True, errors="replace",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(marker.read_text(encoding="utf-8"), "unchanged\n")
        self.assertEqual([marker], list(evidence.iterdir()))

    def test_fresh_image_requires_external_hashes_before_writing_evidence(self) -> None:
        archive = self.write_archive("fresh-preflight.zip", self.base_files())
        evidence = self.root / "fresh-evidence"
        result = subprocess.run(
            [
                "powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(RUNNER),
                "-Scope", "FreshImage", "-ImageReference", "test-image", "-Archive", str(archive),
                "-Probe", str(self.probe), "-EvidenceDirectory", str(evidence),
            ],
            capture_output=True, text=True, errors="replace",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(evidence.exists())
        self.assertIn("externally recorded archive and probe SHA-256", result.stdout + result.stderr)

    def test_hash_mismatch_does_not_create_evidence(self) -> None:
        archive = self.write_archive("hash-mismatch.zip", self.base_files())
        evidence = self.root / "hash-mismatch-evidence"
        result = subprocess.run(
            [
                "powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(RUNNER),
                "-Scope", "RestrictedHost", "-Archive", str(archive), "-Probe", str(self.probe),
                "-ExpectedArchiveSha256", "0" * 64, "-EvidenceDirectory", str(evidence),
            ],
            capture_output=True, text=True, errors="replace",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(evidence.exists())
        self.assertIn("Archive SHA-256 does not match", result.stdout + result.stderr)

    def test_timeout_terminates_child_tree_and_writes_log(self) -> None:
        harness = self.root / "timeout-harness.ps1"
        child_pid = self.root / "child-pid.txt"
        log = self.root / "timeout.log"
        work = self.root / "timeout-work"
        work.mkdir()
        quote = lambda path: str(path).replace("'", "''")
        target_script = self.root / "spawn-and-sleep.ps1"
        target_script.write_text(
            f"$child = Start-Process -FilePath ($env:SystemRoot + '\\System32\\WindowsPowerShell\\v1.0\\powershell.exe') -ArgumentList '-NoProfile -Command \"Start-Sleep -Seconds 60\"' -PassThru\n"
            f"Set-Content -LiteralPath '{quote(child_pid)}' -Value $child.Id\n"
            "Start-Sleep -Seconds 60\n",
            encoding="utf-8",
        )
        harness.write_text(
            f"""
$ErrorActionPreference = 'Stop'
. '{quote(RUNNER)}' -Archive ignored -Probe ignored -EvidenceDirectory ignored -Scope RestrictedHost
$environment = @{{
  SystemRoot = $env:SystemRoot
  WINDIR = $env:SystemRoot
  SystemDrive = $env:SystemDrive
  COMSPEC = ($env:SystemRoot + '\\System32\\cmd.exe')
  PATH = ($env:SystemRoot + '\\System32;' + $env:SystemRoot)
  PATHEXT = '.COM;.EXE;.BAT;.CMD'
  TEMP = '{quote(work)}'
  TMP = '{quote(work)}'
}}
$timedOut = $false
try {{
  Invoke-CleanProcess ($env:SystemRoot + '\\System32\\WindowsPowerShell\\v1.0\\powershell.exe') @('-NoLogo', '-NoProfile', '-File', '{quote(target_script)}') '{quote(work)}' $environment '{quote(log)}' 1 | Out-Null
}}
catch {{
  if ($_.Exception.Message -like 'Process timed out*') {{ $timedOut = $true }} else {{ throw }}
}}
if (-not $timedOut) {{ throw 'The timeout path did not run.' }}
Start-Sleep -Milliseconds 500
$child = [int](Get-Content -LiteralPath '{quote(child_pid)}' -Raw)
if (Get-Process -Id $child -ErrorAction SilentlyContinue) {{ throw "Timed-out child process remains: $child" }}
if ((Get-Content -LiteralPath '{quote(log)}' -Raw) -notmatch 'TIMEOUT after 1 seconds') {{ throw 'Timeout log is incomplete.' }}
"timeout cleanup passed"
""",
            encoding="utf-8",
        )
        result = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(harness)],
            capture_output=True,
            text=True,
            errors="replace",
            timeout=20,
        )
        details = result.stdout + result.stderr
        if log.exists():
            details += "\nLOG\n" + log.read_text(encoding="utf-8-sig", errors="replace")
        self.assertEqual(result.returncode, 0, details)
        self.assertIn("timeout cleanup passed", result.stdout)

    def test_parent_exits_first_child_is_still_job_owned(self) -> None:
        harness = self.root / "parent-exit-harness.ps1"
        child_pid = self.root / "parent-exit-child-pid.txt"
        log = self.root / "parent-exit.log"
        work = self.root / "parent-exit-work"
        work.mkdir()
        quote = lambda path: str(path).replace("'", "''")
        target_script = self.root / "spawn-and-exit.ps1"
        target_script.write_text(
            f"$child = Start-Process -FilePath ($env:SystemRoot + '\\System32\\WindowsPowerShell\\v1.0\\powershell.exe') -ArgumentList '-NoProfile -Command \"Start-Sleep -Seconds 60\"' -PassThru\n"
            f"Set-Content -LiteralPath '{quote(child_pid)}' -Value $child.Id\n",
            encoding="utf-8",
        )
        harness.write_text(
            f"""
$ErrorActionPreference = 'Stop'
. '{quote(RUNNER)}' -Archive ignored -Probe ignored -EvidenceDirectory ignored -Scope RestrictedHost
$environment = @{{
  SystemRoot = $env:SystemRoot
  WINDIR = $env:SystemRoot
  SystemDrive = $env:SystemDrive
  COMSPEC = ($env:SystemRoot + '\\System32\\cmd.exe')
  PATH = ($env:SystemRoot + '\\System32;' + $env:SystemRoot)
  PATHEXT = '.COM;.EXE;.BAT;.CMD'
  TEMP = '{quote(work)}'
  TMP = '{quote(work)}'
}}
try {{
  Invoke-CleanProcess ($env:SystemRoot + '\\System32\\WindowsPowerShell\\v1.0\\powershell.exe') @('-NoLogo', '-NoProfile', '-File', '{quote(target_script)}') '{quote(work)}' $environment '{quote(log)}' 10 | Out-Null
}}
catch {{
  if ($_.Exception.Message -notlike 'The root process exited while an owned descendant*') {{ throw }}
}}
Start-Sleep -Milliseconds 500
$child = [int](Get-Content -LiteralPath '{quote(child_pid)}' -Raw)
if (Get-Process -Id $child -ErrorAction SilentlyContinue) {{ throw "Parent-exit child process remains: $child" }}
if (-not (Test-Path -LiteralPath '{quote(log)}' -PathType Leaf)) {{ throw 'Parent-exit log is missing.' }}
"parent exit cleanup passed"
""",
            encoding="utf-8",
        )
        result = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(harness)],
            capture_output=True,
            text=True,
            errors="replace",
            timeout=25,
        )
        details = result.stdout + result.stderr
        if log.exists():
            details += "\nLOG\n" + log.read_text(encoding="utf-8-sig", errors="replace")
        self.assertEqual(result.returncode, 0, details)
        self.assertIn("parent exit cleanup passed", result.stdout)

    def test_fast_native_root_cannot_launch_before_job_assignment(self) -> None:
        compiler = Path(os.environ["WINDIR"]) / "Microsoft.NET" / "Framework64" / "v4.0.30319" / "csc.exe"
        if not compiler.is_file():
            self.skipTest(f".NET Framework x64 compiler is unavailable: {compiler}")
        source = self.root / "fast-root.cs"
        executable = self.root / "fast-root.exe"
        source.write_text(
            r'''using System;
using System.Diagnostics;
using System.IO;

public static class FastRoot
{
    public static int Main(string[] args)
    {
        string launcher = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.Windows),
            "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
        var child = Process.Start(new ProcessStartInfo {
            FileName = launcher,
            Arguments = "-NoLogo -NoProfile -Command \"Start-Sleep -Seconds 60\"",
            UseShellExecute = false
        });
        File.WriteAllText(args[0], child.Id.ToString());
        return 0;
    }
}
''',
            encoding="utf-8",
        )
        compile_result = subprocess.run(
            [str(compiler), "/nologo", "/target:exe", "/platform:x64", f"/out:{executable}", str(source)],
            capture_output=True,
            text=True,
            errors="replace",
        )
        self.assertEqual(compile_result.returncode, 0, compile_result.stdout + compile_result.stderr)

        harness = self.root / "fast-root-harness.ps1"
        child_pid = self.root / "fast-root-child-pid.txt"
        log = self.root / "fast-root.log"
        work = self.root / "fast-root-work"
        work.mkdir()
        quote = lambda path: str(path).replace("'", "''")
        harness.write_text(
            f"""
$ErrorActionPreference = 'Stop'
. '{quote(RUNNER)}' -Archive ignored -Probe ignored -EvidenceDirectory ignored -Scope RestrictedHost
$environment = @{{
  SystemRoot = $env:SystemRoot
  WINDIR = $env:SystemRoot
  SystemDrive = $env:SystemDrive
  COMSPEC = ($env:SystemRoot + '\\System32\\cmd.exe')
  PATH = ($env:SystemRoot + '\\System32;' + $env:SystemRoot)
  PATHEXT = '.COM;.EXE;.BAT;.CMD'
  TEMP = '{quote(work)}'
  TMP = '{quote(work)}'
}}
try {{
  Invoke-CleanProcess '{quote(executable)}' @('{quote(child_pid)}') '{quote(work)}' $environment '{quote(log)}' 10 | Out-Null
}}
catch {{
  if ($_.Exception.Message -notlike 'The root process exited while an owned descendant*') {{ throw }}
}}
Start-Sleep -Milliseconds 500
$child = [int](Get-Content -LiteralPath '{quote(child_pid)}' -Raw)
if (Get-Process -Id $child -ErrorAction SilentlyContinue) {{ throw "Fast-root child escaped the process job: $child" }}
"fast native root cleanup passed"
""",
            encoding="utf-8",
        )
        result = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(harness)],
            capture_output=True,
            text=True,
            errors="replace",
            timeout=25,
        )
        details = result.stdout + result.stderr
        if log.exists():
            details += "\nLOG\n" + log.read_text(encoding="utf-8-sig", errors="replace")
        self.assertEqual(result.returncode, 0, details)
        self.assertIn("fast native root cleanup passed", result.stdout)


if __name__ == "__main__":
    unittest.main()
