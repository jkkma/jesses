# SVT provider identities

The source Quick Convert, AV1AN and batch UI exposes one SVT-AV1 choice. Canonical
`svtAv1` resolves the fork-qualified pinned SVT-AV1-HDR provider, matching Nmkoder's
bundle recipe. It supports the source's scalar and managed HDR/grain resource
rows without a visible build selector. Source CRF0 is literal zero and carries
no invented lossless guarantee. See [source SVT qualification](source-svt-provider.md)
for exact pins, commands, target defaults, recovery safeguards and platform limits.

Existing internal/native `svtAv1FiveFish` and `svtAv1Hdr` jobs retain their identities.
The original explicit `JESSES_SVT_AV1` override remains available for intentional
legacy/custom mainline use. Defaults never silently choose an unqualified mainline
provider, and saved-settings resume never substitutes different executable bytes.

The Windows installer is `scripts/install-svt-forks.ps1`, with `-Fork 5fish` or
`-Fork hdr`. It verifies pinned archives and executable hashes and preserves
notices/provenance in separate managed directories. It changes neither PATH nor
an existing different installation. Exact versions and platform assets are in
[scripts/svt-forks.json](../scripts/svt-forks.json). The pinned x86-64-v3 assets
require an appropriate CPU. There is no source prebuilt macOS HDR asset.

Managed roots are `%LOCALAPPDATA%/jesses/tools` on Windows,
`$XDG_DATA_HOME/jesses/tools` (default `~/.local/share/jesses/tools`) on Linux,
and `~/Library/Application Support/jesses/tools` on macOS. Provider directories
remain `svt-av1-hdr` and `svt-av1-5fish`. Explicit HDR/FiveFish override variables
and distinct PATH aliases remain `JESSES_SVT_AV1_HDR`/`SvtAv1EncApp-HDR` and
`JESSES_SVT_AV1_5FISH`/`SvtAv1EncApp-5fish`. Invalid configured installations fail
visibly; the canonical source path also verifies the exact qualified HDR marker.
