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
