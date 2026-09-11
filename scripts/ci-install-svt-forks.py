"""Install the manifest's pinned Linux forks for the native CI gate."""

import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
from urllib.request import urlopen


def download(url: str) -> bytes:
    with urlopen(url, timeout=60) as response:
        return response.read()


def main() -> None:
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise SystemExit("This CI installer requires Linux x86-64.")
    flags = Path("/proc/cpuinfo").read_text().split("flags", 1)[1].splitlines()[0]
    if not {"avx2", "bmi2", "fma"}.issubset(flags.split()):
        raise SystemExit("The pinned upstream builds require an x86-64-v3 runner.")
    manifest = json.loads(Path(__file__).with_name("svt-forks.json").read_text())
    root = Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir())).resolve()
    install = Path(tempfile.mkdtemp(prefix="jesses-svt-forks-", dir=root))
    for encoder in manifest["encoders"]:
        asset = encoder["assets"]["linuxX86_64"]
        directory = install / encoder["id"]
        directory.mkdir()
        archive = directory / "release.tar.xz"
        payload = download(asset["url"])
        if hashlib.sha256(payload).hexdigest() != asset["sha256"]:
            raise RuntimeError(f"Archive checksum mismatch for {encoder['id']}")
        archive.write_bytes(payload)
        with tarfile.open(archive) as package:
            members = package.getmembers()
            if len(members) != 1 or members[0].name != asset["entry"] or not members[0].isfile():
                raise RuntimeError(f"Unexpected archive structure for {encoder['id']}")
            source = package.extractfile(members[0])
            if source is None:
                raise RuntimeError("Archive executable is missing")
            executable = directory / asset["entry"]
            executable.write_bytes(source.read())
        executable.chmod(0o755)
        result = subprocess.run([executable, "--version"], capture_output=True, text=True, timeout=10, check=True)
        version = result.stdout + result.stderr
        if encoder["versionMarker"] not in version:
            raise RuntimeError(f"Unexpected executable identity: {version}")
        for license_name in encoder["licenses"]:
            (directory / license_name).write_bytes(download(encoder["licenseBaseUrl"] + license_name))
        (directory / "provenance.json").write_text(json.dumps(encoder, indent=2))
        with Path(os.environ["GITHUB_ENV"]).open("a") as environment:
            environment.write(f"{encoder['environmentVariable']}={executable}\n")
        print(f"Verified {encoder['id']}: {version.strip()}")


if __name__ == "__main__":
    main()
