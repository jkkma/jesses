#!/bin/sh
set -eu

export LC_ALL=C
pattern='Stax''Rip'

reject_matches() {
    message=$1
    shift
    if "$@"; then
        printf '%s\n' "$message" >&2
        return 1
    else
        status=$?
        if [ "$status" -ne 1 ]; then
            return "$status"
        fi
    fi
}

check_tree() {
    reject_matches 'Remove the restricted name from the tracked files listed above.' \
        git grep -a -i -F -l -e "$pattern" "$1" --
}

case "${1-}" in
    --staged)
        reject_matches 'Remove the restricted name from the staged files listed above.' \
            git grep --cached -a -i -F -l -e "$pattern" --
        ;;
    --tree)
        check_tree "$2"
        ;;
    --message)
        reject_matches 'Remove the restricted name from the commit message.' \
            grep -a -i -F -q -e "$pattern" -- "$2"
        ;;
    --commits)
        matches=$(git log --format=%h --no-notes --fixed-strings --regexp-ignore-case \
            --grep="$pattern" "$2" --)
        if [ -n "$matches" ]; then
            printf 'Remove the restricted name from the messages of these commits:\n%s\n' "$matches" >&2
            exit 1
        fi
        commits=$(git rev-list "$2" --)
        for commit in $commits; do
            check_tree "$commit"
        done
        ;;
    *)
        printf 'Usage: %s --staged | --tree <revision> | --message <file> | --commits <range>\n' "$0" >&2
        exit 2
        ;;
esac
