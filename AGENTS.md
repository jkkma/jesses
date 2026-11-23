# Repository Git identity

For commits made on behalf of the repository owner, use `jkkma` and
`16640398+jkkma@users.noreply.github.com` for both author and committer.
Keep Git identity settings local to this repository; do not change global settings.

Before publishing, inspect both identities in every new or rewritten commit.
Repository-local Git configuration does not control identities chosen by a remote API.
Use a Git transport that publishes the inspected local objects, or an API that lets
you explicitly set both identities. The connected GitHub `create_commit` tool lacks
those fields: do not use its server defaults to create public commits.
If an identity-preserving publication route is unavailable, report that limitation
instead of publishing with an uncontrolled identity.

Preserve other contributors' identities. Rewrite existing owner metadata only when
the user has authorized it, and preserve the corresponding source trees and work.

## Session disk usage and cleanup

- Check checkout and `target` sizes and available disk space before and after
  large builds and before ending each session.
- Reuse suitable build directories. When practical, set `CARGO_INCREMENTAL=0`
  for one-off Rust qualification builds.
- Once builds finish and no active jobs need the files, retire stale generated
  caches, PDBs, test binaries, and duplicate package trees by deleting them
  directly. If direct deletion is unavailable, move them to the Windows Recycle
  Bin in batches that fit its quota and tell the user about that fallback.
- Keep source files and edits, user media and data, unique outputs, compact
  verification receipts, and files required by active work.
- Perform size checks and session cleanup silently by default.
