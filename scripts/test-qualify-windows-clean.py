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

        cls.native_fixture_temporary = tempfile.TemporaryDirectory(prefix="jesses-windows-clean-native-")
        fixture_root = Path(cls.native_fixture_temporary.name)
        compiler = Path(os.environ["WINDIR"]) / "Microsoft.NET" / "Framework64" / "v4.0.30319" / "csc.exe"
        cls.native_fixture = fixture_root / "clean-process-fixture.exe"
        cls.native_fixture_error = ""
        if not compiler.is_file():
            cls.native_fixture_error = f".NET Framework x64 compiler is unavailable: {compiler}"
            return

        source = fixture_root / "clean-process-fixture.cs"
        source.write_text(
            r'''using System;
using System.Diagnostics;
using System.IO;
using System.Threading;

public static class CleanProcessFixture
{
    private static Process StartHiddenChild()
    {
        string launcher = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.Windows),
            "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
        return Process.Start(new ProcessStartInfo {
            FileName = launcher,
            Arguments = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -Command \"Start-Sleep -Seconds 60\"",
            UseShellExecute = false,
            CreateNoWindow = true,
            WindowStyle = ProcessWindowStyle.Hidden
        });
    }

    public static int Main(string[] args)
    {
        if (args.Length == 4 && args[0] == "echo") {
            File.WriteAllText(args[1], args[2] + Environment.NewLine + args[3] + Environment.NewLine);
            Console.WriteLine("native arguments received");
            return 0;
        }
        if (args.Length != 3 || (args[0] != "spawn-wait" && args[0] != "spawn-exit")) {
            Console.Error.WriteLine("Expected echo or spawn mode arguments.");
            return 64;
        }

        Process child = StartHiddenChild();
        File.WriteAllText(args[1], child.Id.ToString());
        File.WriteAllText(args[2], "ready" + Environment.NewLine);
        Console.WriteLine("child ready: " + child.Id);
        if (args[0] == "spawn-wait") {
            Thread.Sleep(TimeSpan.FromSeconds(60));
        }
        return 0;
    }
}
''',
            encoding="utf-8",
        )
        compile_result = subprocess.run(
            [str(compiler), "/nologo", "/target:exe", "/platform:x64", f"/out:{cls.native_fixture}", str(source)],
            capture_output=True,
            text=True,
            errors="replace",
        )
        if compile_result.returncode != 0:
            details = compile_result.stdout + compile_result.stderr
            cls.native_fixture_temporary.cleanup()
            raise RuntimeError(f"Could not compile native clean-process fixture:\n{details}")

    @classmethod
    def tearDownClass(cls) -> None:
        cls.native_fixture_temporary.cleanup()

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="jesses-windows-clean-test-")
        self.root = Path(self.temporary.name)
        self.probe = self.root / "package_tools.exe"
        self.probe.write_bytes(b"MZ test probe; malformed archives fail before execution\n")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def require_native_fixture(self) -> Path:
        if self.native_fixture_error:
            self.skipTest(self.native_fixture_error)
        return self.native_fixture

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
        executable = self.require_native_fixture()
        harness = self.root / "timeout-harness.ps1"
        child_pid = self.root / "child-pid.txt"
        child_ready = self.root / "child-ready.txt"
        log = self.root / "timeout.log"
        work = self.root / "timeout-work"
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
$timedOut = $false
try {{
  Invoke-CleanProcess '{quote(executable)}' @('spawn-wait', '{quote(child_pid)}', '{quote(child_ready)}') '{quote(work)}' $environment '{quote(log)}' 10 | Out-Null
}}
catch {{
  if ($_.Exception.Message -like 'Process timed out*') {{ $timedOut = $true }} else {{ throw }}
}}
if (-not $timedOut) {{ throw 'The timeout path did not run.' }}
if (-not (Test-Path -LiteralPath '{quote(child_ready)}' -PathType Leaf)) {{ throw 'Native child fixture never reached its readiness marker.' }}
if (-not (Test-Path -LiteralPath '{quote(child_pid)}' -PathType Leaf)) {{ throw 'Native child fixture did not record its child PID.' }}
$child = [int](Get-Content -LiteralPath '{quote(child_pid)}' -Raw)
$deadline = [DateTime]::UtcNow.AddSeconds(5)
while (Get-Process -Id $child -ErrorAction SilentlyContinue) {{
  if ([DateTime]::UtcNow -ge $deadline) {{ throw "Timed-out child process remains: $child" }}
  Start-Sleep -Milliseconds 50
}}
if ((Get-Content -LiteralPath '{quote(log)}' -Raw) -notmatch 'TIMEOUT after 10 seconds') {{ throw 'Timeout log is incomplete.' }}
"timeout cleanup passed"
""",
            encoding="utf-8",
        )
        result = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(harness)],
            capture_output=True,
            text=True,
            errors="replace",
            timeout=35,
        )
        details = result.stdout + result.stderr
        if log.exists():
            details += "\nLOG\n" + log.read_text(encoding="utf-8-sig", errors="replace")
        self.assertEqual(result.returncode, 0, details)
        self.assertIn("timeout cleanup passed", result.stdout)

    def test_clean_process_forwards_native_arguments_and_writes_log(self) -> None:
        executable = self.require_native_fixture()
        harness = self.root / "native-arguments-harness.ps1"
        received = self.root / "native arguments received.txt"
        log = self.root / "native-arguments.log"
        work = self.root / "native arguments work"
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
$result = Invoke-CleanProcess '{quote(executable)}' @('echo', '{quote(received)}', 'argument with spaces', 'C:\\Program Files\\Qualification Tool\\probe.exe') '{quote(work)}' $environment '{quote(log)}' 10
if ($result.stdout -notmatch 'native arguments received') {{ throw 'Native stdout was not captured.' }}
$expected = @('argument with spaces', 'C:\\Program Files\\Qualification Tool\\probe.exe') -join [Environment]::NewLine
$expected += [Environment]::NewLine
if ((Get-Content -LiteralPath '{quote(received)}' -Raw) -cne $expected) {{ throw 'Native arguments were not preserved.' }}
if ((Get-Content -LiteralPath '{quote(log)}' -Raw) -notmatch 'native arguments received') {{ throw 'Native success log is incomplete.' }}
"native argument forwarding passed"
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
        self.assertIn("native argument forwarding passed", result.stdout)

    def test_fast_native_root_cannot_launch_before_job_assignment(self) -> None:
        executable = self.require_native_fixture()
        harness = self.root / "fast-root-harness.ps1"
        child_pid = self.root / "fast-root-child-pid.txt"
        child_ready = self.root / "fast-root-child-ready.txt"
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
  Invoke-CleanProcess '{quote(executable)}' @('spawn-exit', '{quote(child_pid)}', '{quote(child_ready)}') '{quote(work)}' $environment '{quote(log)}' 10 | Out-Null
}}
catch {{
  if ($_.Exception.Message -notlike 'The root process exited while an owned descendant*') {{ throw }}
}}
if (-not (Test-Path -LiteralPath '{quote(child_ready)}' -PathType Leaf)) {{ throw 'Fast native fixture never reached its readiness marker.' }}
if (-not (Test-Path -LiteralPath '{quote(child_pid)}' -PathType Leaf)) {{ throw 'Fast native fixture did not record its child PID.' }}
$child = [int](Get-Content -LiteralPath '{quote(child_pid)}' -Raw)
$deadline = [DateTime]::UtcNow.AddSeconds(5)
while (Get-Process -Id $child -ErrorAction SilentlyContinue) {{
  if ([DateTime]::UtcNow -ge $deadline) {{ throw "Fast-root child escaped the process job: $child" }}
  Start-Sleep -Milliseconds 50
}}
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
