# Third-party notices

jesses is created by jkkma and licensed under GPL-3.0-only.

The Button source in `src/lib/components/ui/button` was generated from
[shadcn-svelte](https://github.com/huntabyte/shadcn-svelte), version 1.6.1,
and adapted for the jesses theme. Its MIT notice is preserved in
`licenses/shadcn-svelte-MIT.txt`.

Frontend and Rust dependency versions are recorded in `pnpm-lock.yaml` and
`Cargo.lock`. Their respective licenses continue to apply.

Media tools are external executables. This development build discovers tools
from PATH and does not distribute FFmpeg, FFprobe, av1an, or standalone encoders.
Any future bundled tool distribution requires its own source, version, hash,
and license record.
