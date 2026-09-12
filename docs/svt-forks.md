# SVT builds for anime and HDR movies

Jesses supports three explicit SVT builds in Quick Convert, av1an, and Batch
encode. Selecting a build selects its executable and its supported tuning
controls. Existing `svtAv1` job history still denotes the original generic SVT
choice; new fork jobs retain `svtAv1FiveFish` or `svtAv1Hdr` across restarts.

SVT-AV1-HDR is the default in Quick Convert, av1an, and Batch encode. Choose
5fish for anime; mainline SVT-AV1 remains an additional option. Default selection
does not turn on HDR10 fallback or reinterpret existing saved job identities.

| Build                                                    | Fresh settings                          | Additional controls                                              |
| -------------------------------------------------------- | --------------------------------------- | ---------------------------------------------------------------- |
| SVT-AV1                                                  | CRF 30, preset 4                        | Grain synthesis, HDR10 fallback                                  |
| [5fish/SVT-AV1](https://github.com/5fish/SVT-AV1)        | CRF 18, preset 2, line-art 5, texture 4 | Paired anime bias controls, each 0–7                             |
| [SVT-AV1-HDR](https://github.com/juliobbv-p/svt-av1-hdr) | CRF 30, preset 2, film grain retention  | Visual quality (`--tune 0`) or film grain retention (`--tune 5`) |

Film grain retention is distinct from AV1 grain synthesis. Synthesis starts at
zero and encoder denoising remains disabled. Choosing HDR does not enable HDR10
fallback. The current output contract preserves validated static HDR10; Dolby
Vision/HDR10+ dynamic metadata still requires explicit supported HDR10 fallback.
The fork's optional dynamic metadata CLI features are not integrated yet.

Jesses currently exposes integer CRF 1–63 and presets 0–13. Extended/fractional
CRF, research presets, and further fork-specific controls remain future work.
Defaults only initialize fresh drafts; switching sources, builds, and workflows
restores the previous draft. No filename-based anime classification is applied.

## Install on Windows

From the repository, run with x64 PowerShell 7:

```powershell
pwsh -File scripts/install-svt-forks.ps1
```

Use `-Fork 5fish` or `-Fork hdr` to install one build. The pinned official release
assets require an x86-64-v3 CPU, including AVX2, BMI2, and FMA. The installer checks
published archive and executable SHA-256 hashes and version signatures, retains
licenses and provenance, and places each build in its own directory:

```text
%LOCALAPPDATA%\jesses\tools\svt-av1-5fish\SvtAv1EncApp.exe
%LOCALAPPDATA%\jesses\tools\svt-av1-hdr\SvtAv1EncApp.exe
```

The installer does not change PATH or overwrite a different existing install.
Open Tools and refresh capability checks after installation. The pinned URLs,
commits, and hashes are in [svt-forks.json](../scripts/svt-forks.json). These builds
are optional external tools; they are not included in the Jesses app bundle.

## Custom builds and other platforms

Each fork resolves in this order: its explicit environment override, its managed
directory, then its distinct PATH alias. An invalid override or managed install
fails visibly instead of falling through. Overrides must be absolute native
executable paths:

- `JESSES_SVT_AV1_5FISH`
- `JESSES_SVT_AV1_HDR`

Managed directories use `$XDG_DATA_HOME/jesses/tools` (default
`~/.local/share/jesses/tools`) on Linux, or
`~/Library/Application Support/jesses/tools` on macOS, followed by the build ID
and `SvtAv1EncApp`. Install a native build appropriate for the platform and CPU;
the Windows installer does not install macOS builds.

Distinct PATH aliases are `SvtAv1EncApp-5fish` and `SvtAv1EncApp-HDR` (with `.exe`
on Windows). For av1an, use the canonical `SvtAv1EncApp` filename in separate
directories. Generic `SvtAv1EncApp` on PATH remains the mainline selection. Known
fork signatures at that generic path are reported as mismatches; Jesses cannot
identify an implementation from the filename alone.

## Validation

Unit and frontend checks cover fork identity, settings isolation, history,
HDR fallback consent, and build-specific arguments. Native integration checks
encode small SDR and HDR fixtures, validate every decoded frame and copied
track through the job pipeline, check the reported build, and reopen saved jobs.

```powershell
cargo test -p media-runtime --test svt_forks standalone_forks -- --ignored
cargo test -p media-runtime --test svt_forks av1an_uses -- --ignored
```

The av1an gate also requires av1an, VapourSynth, and L-SMASH Works. Linux CI
downloads the same pinned forks, verifies their archive hashes and identities,
and runs the standalone gate. Windows and Linux unit checks exercise isolated
child PATH handling without mutating the application's environment.
