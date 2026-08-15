#!/bin/bash
# Is every resource this app defines reachable from something that runs?
#
# Nothing warns when the last reference to a resource goes: a screen that is
# replaced leaves its strings and its icons behind, and every build afterwards
# compiles them in. The question has to be *reachability* rather than "named
# somewhere" — a drawable named only by a layout nobody inflates is as dead as
# one named by nothing at all — so this walks out from the code and the
# manifest, follows every reference it finds, and reports what it never arrives
# at.
#
#   ./check-resources.sh
#
# A name built at run time would defeat it, so the one API that can do that is
# refused outright rather than worked around.
set -euo pipefail
cd "$(dirname "$0")/../app"

if git grep -n 'getIdentifier' -- 'src/*.java' >&2; then
    echo "check-resources: a resource named at run time cannot be checked" >&2
    exit 1
fi

# One awk over every file at once: reachability is a property of the whole
# graph, so a file list split across two runs would answer a different question.
mapfile -t files < <(git ls-files src/main/AndroidManifest.xml 'src/*.java' src/main/res)

report=$(awk '
    # A file under res/ is itself a resource, and is the owner of every
    # reference in it; values/ files hold many, so ownership there is per
    # element and tracked below. Anything else — the Java, the manifest — is a
    # root, whose owner is the empty string.
    function typeOf(elem) {
        if (elem ~ /^(string|plurals|style|color|dimen|bool|integer|fraction|attr)$/) return elem
        if (elem ~ /-?array$/) return "array"
        return ""
    }
    # A dot in a resource name is an underscore in R, so the two spellings are
    # one resource and the key has to be the same either way. aapt has the same
    # collision and resolves it the same way.
    function key(t, n) { gsub(/\./, "_", n); return t "/" n }
    function use(k) { ne++; from[ne] = owner; to[ne] = k }

    FNR == 1 {
        owner = ""; values = 0
        if (FILENAME ~ /\/res\/values[^\/]*\//) {
            values = 1
        } else if (FILENAME ~ /\/res\//) {
            type = FILENAME; sub(/\/[^\/]*$/, "", type); sub(/.*\//, "", type)
            sub(/-.*/, "", type)                      # a qualifier is not a type
            name = FILENAME; sub(/.*\//, "", name); sub(/\..*$/, "", name)
            owner = key(type, name)
            def[owner] = FILENAME
        }
    }

    # A style whose name is A.B inherits from A unless it says otherwise, and
    # the parent attribute is often on the next line — hence the open-tag state
    # rather than a per-line test.
    values && open {
        if (match($0, /parent="/)) parent = 1
        if (match($0, /[\/>]/)) {
            if (!parent && sub(/\.[^.]+$/, "", styleName)) use(key("style", styleName))
            open = 0
        }
    }

    values {
        if (match($0, /<[a-z-]+[ \t]+[^>]*name="[^"]+"/)) {
            tag = substr($0, RSTART, RLENGTH)
            elem = tag; sub(/^</, "", elem); sub(/[ \t].*/, "", elem)
            nm = tag; sub(/.*name="/, "", nm); sub(/".*/, "", nm)
            t = typeOf(elem)
            if (elem == "item" && tag ~ /type="/) {     # <item type="id" …/>
                t = tag; sub(/.*type="/, "", t); sub(/".*/, "", t)
            }
            if (t != "") {
                owner = key(t, nm)
                def[owner] = FILENAME
                if (elem == "style") { open = 1; parent = 0; styleName = nm }
            }
        }
        if (match($0, /parent="[^@"][^"]*"/)) {          # parent="A.B", not @style/
            p = substr($0, RSTART, RLENGTH)
            sub(/parent="/, "", p); sub(/"$/, "", p)
            if (p !~ /:/ && p != "") use(key("style", p))
        }
    }

    # @type/name, ?attr/name. A package qualifier means somebody else owns it,
    # and @+id/name is a definition rather than a use.
    {
        s = $0
        while (match(s, /[@?]\+?([A-Za-z_][A-Za-z0-9_]*:)?[a-z]+\/[A-Za-z0-9_.]+/)) {
            tok = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH)
            if (tok ~ /^.\+/ || tok ~ /:/) continue
            sub(/^[@?]/, "", tok)
            t = tok; sub(/\/.*/, "", t); sub(/.*\//, "", tok)
            use(key(t, tok))
        }
    }

    FILENAME ~ /\.java$/ {
        s = $0
        while (match(s, /(^|[^A-Za-z0-9_.])R\.[a-z]+\.[A-Za-z0-9_]+/)) {
            tok = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH)
            sub(/.*R\./, "", tok)
            t = tok; sub(/\..*/, "", t); sub(/.*\./, "", tok)
            use(key(t, tok))
        }
    }

    END {
        live[""] = 1
        do {
            changed = 0
            for (i = 1; i <= ne; i++)
                if (live[from[i]] && !live[to[i]]) { live[to[i]] = 1; changed = 1 }
        } while (changed)

        for (k in def) {
            n++
            if (!live[k]) printf "%s: unreachable: %s\n", def[k], k
        }
        printf "%d resources\n", n > "/dev/stderr"
    }
' "${files[@]}" | sort)

if [ -n "$report" ]; then
    printf '%s\n' "$report"
    exit 1
fi
echo "ok"
