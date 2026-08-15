#!/bin/sh
# Patch management for the third_party/ submodules.
#
# Submodule gitlinks always point at the pristine upstream commit; local
# changes live in patches/<name>/*.patch. For each submodule, this script
# tags the pinned commit as `base`, applies the patches with `git am` when
# the submodule is still sitting at base, and (unless -n) regenerates the
# patch files from base..HEAD with `git format-patch`.
set -eu

nosave=

usage() {
    echo "usage: $0 [-n] [submodule...]"
    echo "       $0 check"
    echo
    echo "options:"
    echo "  -h  show this help text"
    echo "  -n  do not re-generate patch files"
    echo
    echo "check does not modify anything; it exits nonzero if any submodule"
    echo "with patches does not have them applied (used by gradle preBuild)"
    exit 2
}

while getopts ":hn" arg; do
    case "$arg" in
        n) nosave=1 ;;
        h|*) usage ;;
    esac
done
shift $((OPTIND - 1))

cd "$(dirname "$0")"

mode=patch
if [ "${1-}" = check ]; then
    mode=check
    shift
    [ $# -eq 0 ] || usage
fi

paths=$(git config -f .gitmodules --get-regexp '^submodule\..*\.path$' | cut -d' ' -f2)

filter="$*"
for name in $filter; do
    found=
    for path in $paths; do
        [ "${path##*/}" = "$name" ] && found=1
    done
    if [ -z "$found" ]; then
        echo "error: no submodule named $name" >&2
        exit 1
    fi
done

status=0
for path in $paths; do
    name=${path##*/}
    if [ -n "$filter" ]; then
        case " $filter " in
            *" $name "*) ;;
            *) continue ;;
        esac
    fi

    set -- "patches/$name"/*.patch
    havepatches=
    [ -e "$1" ] && havepatches=1

    # the pinned upstream commit, as recorded by the gitlink
    base=$(git rev-parse --verify ":$path")

    if ! [ -e "$path/.git" ]; then
        if [ "$mode" = patch ] || [ -n "$havepatches" ]; then
            echo "error: submodule $name is not initialized; run: git submodule update --init" >&2
            exit 1
        fi
        continue
    fi

    gitdir=$(git -C "$path" rev-parse --absolute-git-dir)
    head=$(git -C "$path" rev-parse HEAD)

    if [ -d "$gitdir/rebase-apply" ]; then
        printf '\033[1;34m> %s\033[0m\n' "$name: patch application in progress"
        git -C "$path" status
        exit 1
    fi

    if [ "$mode" = check ]; then
        if [ -n "$havepatches" ] && [ "$head" = "$base" ]; then
            echo "error: submodule $name has unapplied patches; run: ./patch.sh" >&2
            status=1
        fi
        # A gitlink always names the pristine upstream commit, and `git add -A`
        # in the outer repository happily records the patched one instead —
        # after which the patches look applied to nothing and the next clone
        # gets them twice. The base tag is what remembers which is which.
        if tagged=$(git -C "$path" rev-parse -q --verify 'refs/tags/base^{commit}'); then
            if [ "$tagged" != "$base" ]; then
                echo "error: submodule $name is pinned at $base, which is not the upstream commit its patches apply to ($tagged)" >&2
                echo "hint: git update-index --cacheinfo 160000,$tagged,$path" >&2
                status=1
            fi
        fi
        continue
    fi

    # ensure the base tag exists at the pinned commit
    if tagged=$(git -C "$path" rev-parse -q --verify 'refs/tags/base^{commit}'); then
        if [ "$tagged" != "$base" ]; then
            echo "error: $name: tag base is at $tagged, but the pinned commit is $base" >&2
            echo "hint: rebase local commits onto the new pin, then: git -C $path tag -f base $base" >&2
            exit 1
        fi
    else
        git -C "$path" tag base "$base"
    fi

    # apply patches
    if [ -n "$havepatches" ] && [ "$head" = "$base" ]; then
        printf '\033[1;34m> %s\033[0m\n' "$name: applying patches"
        git -c user.name=user -c user.email=email -C "$path" am "$PWD/patches/$name"/*.patch
        head=$(git -C "$path" rev-parse HEAD)
    fi

    # regenerate patch files
    if [ -z "$nosave" ] && [ "$head" != "$base" ]; then
        printf '\033[1;34m> %s\033[0m\n' "$name: saving patches"
        rm -rf "patches/$name"
        git -C "$path" format-patch --output-directory "$PWD/patches/$name" \
            --no-stat --no-signature --numbered --always base
    fi
done

exit $status
