# Repository Git hooks

In a checkout without a custom hooks path, run `git config --local core.hooksPath .githooks` to enable the repository name checks. If a custom hooks path is already in use, have its `pre-commit` and `commit-msg` hooks call the corresponding scripts here.

The hooks check the staged tree and the proposed commit message. The Checks workflow scans the event tree and checks the files and message of each new commit in the push or pull request range. For new refs it uses the default branch as the baseline.
