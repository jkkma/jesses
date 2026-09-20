# Windows clean-environment qualification

Use `scripts/qualify-windows-clean.ps1` for the Windows x64 portable ZIP. The
runner verifies the archive manifest and payload hashes, extracts into a
Scoop-shaped disposable tree, and makes the program tree read/execute-only. Its
persisted `jesses-data` target remains writable. It then uses a native discovery
probe and packaged FFmpeg, x264, and SVT-AV1 binaries with a cleared environment,
a system-only `PATH`, and a new profile. The media checks generate their own
eight-frame source; no source media is read or changed.

The receipt also requires a machine- or user-installed Microsoft Edge WebView2
Runtime. The portable package does not contain or install WebView2. The native
application is not launched by this runner, so GUI startup, browser rendering,
accessibility, and interactive workflows require a separate native UI pass.

## Evidence levels

`FreshImage` is valid only inside a newly provisioned disposable Windows image.
The runner requires an image reference and rejects visible media tools, an
existing Jesses managed-tool directory, and inherited `JESSES_*` overrides. A
script cannot prove that an operator reset a VM, so retain the base-image or
snapshot identity with the JSON receipt and discard or revert the image after
the run.

`RestrictedHost` runs the same child-process boundary on an existing Windows
machine. It records ambient tools but removes them from the child environment.
This is useful for checking package isolation and the runner itself; it is not
fresh-machine evidence.

## Inputs

Copy these three inputs into the disposable image or a read-only mapped folder:

- the exact release-candidate portable ZIP;
- `scripts/qualify-windows-clean.ps1` from the same reviewed revision;
- the x64 `package_tools.exe` example built from that revision.

Build the probe before transferring the inputs:

```powershell
cargo build -p media-runtime --example package_tools --release --locked --target x86_64-pc-windows-msvc
```

Record the candidate ZIP and probe hashes outside the disposable image. Inside a
fresh image, run from Windows PowerShell or PowerShell 7 and write evidence to a
new directory:

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\qualify-windows-clean.ps1 `
  -Scope FreshImage `
  -ImageReference "Windows base image and snapshot identifier" `
  -Archive .\jesses-portable.zip `
  -Probe .\package_tools.exe `
  -ExpectedArchiveSha256 <64-character-sha256> `
  -ExpectedProbeSha256 <64-character-sha256> `
  -EvidenceDirectory C:\qualification\jesses-windows-clean
```

The evidence destination must not exist. The runner never deletes or overwrites
it. Preserve `qualification.json` and the `logs` directory together. The extracted
program tree intentionally retains its restricted ACL, and generated fixtures stay
under the disposable persisted-data tree.

For Windows Sandbox, enable and provision Sandbox outside this procedure, map the
input folder read-only, map a separate empty output folder writable, and run the
same command with `FreshImage`. Do not map a developer profile, package manager,
or tool directory. For Hyper-V or another VM, start from a named clean snapshot,
disable shared user-profile folders, copy only the three inputs, run the command,
export the evidence, and revert the snapshot.

To exercise the isolation boundary on a development machine without making a
clean-machine claim:

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts\qualify-windows-clean.ps1 `
  -Scope RestrictedHost `
  -Archive <portable-zip> `
  -Probe <package_tools.exe> `
  -ExpectedArchiveSha256 <64-character-sha256> `
  -ExpectedProbeSha256 <64-character-sha256> `
  -EvidenceDirectory <new-evidence-directory>
```

Before staging a candidate, run the focused Windows PowerShell 5.1 safety tests:

```powershell
python scripts\test-qualify-windows-clean.py
```

Passing checks cover exact archive integrity, packaged discovery of FFmpeg,
FFprobe, x264, mainline SVT-AV1, av1an, and both SVT forks, WebView2 presence,
generated x264/SVT-AV1 encode and decode, writable persisted data, read-only
program resources, and unchanged package/archive bytes. The runner does not cover
Scoop PATH registration, the Scoop update/uninstall lifecycle, native GUI use,
real-media preservation, cancellation or recovery, GPU encoders, AOM, or x265.
