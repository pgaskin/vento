#!/bin/bash
# SPDX-License-Identifier: CC0-1.0
#
# Do these declarations still match the library they bind to?
#
# Everything in this module is linkage rather than design: the natives bind by
# mangled name, and the callbacks are looked up by string and cached as method
# ids. A rename, a moved class or a changed parameter type does not fail to
# compile — it fails at run time, as an UnsatisfiedLinkError if you are lucky and
# a NoSuchMethodError on a native thread if you are not.
#
# So ask the two authorities, and never a decompiler's idea of Java:
#
#   1. every `native` method declared here has an exported Java_… symbol in
#      libvncviewer.so;
#   2. every member declared here whose name also exists in the viewer's own dex
#      has the same descriptor and the same static-ness there.
#
# Either one fails the check. What does *not* fail it is a member that exists
# here and not there: this module is not a copy of their Java, so a record's
# accessors, a constant moved to where its type is and a stub that answers a
# callback are all ours, and are listed rather than complained about. Members
# that exist there and not here are the deliberate subset — their connection
# store, their cloud account, and everything neither side calls.
#
# Both sides come from this module's own pin: the APK it names and the library
# unpacked out of it — the APK from `apk/`, or the copy of that same build in
# the repository's `stuff/`, and nothing else.
#
#   ./check-jni-abi.sh            # builds the module if needed
#   NOBUILD=1 ./check-jni-abi.sh  # against whatever is already built
set -euo pipefail
cd "$(dirname "$0")/../realvnc"

APKTOOL="${APKTOOL:-java -jar $HOME/sdk/apktool_3.0.3.jar}"
PKG=com/realvnc/vncviewer/jni

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ "${NOBUILD:-0}" != 1 ]; then
    # A wrapper builds whatever is in the directory it is run from, so go to it:
    # here that is the repository's one build, with this module a subproject of
    # it; lifted out, it is the wrapper beside this script and this is the root.
    gradlew=""
    for candidate in ./gradlew ../gradlew; do
        [ -x "$candidate" ] && { gradlew="$candidate"; break; }
    done
    [ -n "$gradlew" ] || { echo "no gradlew found; use NOBUILD=1" >&2; exit 1; }
    dir="$(cd "$(dirname "$gradlew")" && pwd)"
    task=':realvnc-jni:assembleDebug'
    [ "$dir" = "$(pwd)" ] && task=':assembleDebug'
    (cd "$dir" && ./gradlew --quiet "$task")
fi

CLASSES="$(find build -type d -path "*/classes/$PKG" 2>/dev/null | head -1)"
[ -n "$CLASSES" ] || { echo "nothing built: run without NOBUILD=1" >&2; exit 1; }

SO="$(find build/generated/vncNativeLibs -name libvncviewer.so 2>/dev/null | head -1)"
[ -n "$SO" ] || { echo "no libvncviewer.so unpacked: see README.md" >&2; exit 1; }

# The pinned build by name, not the first APK lying about: `stuff/` holds the
# 4.9.1 the reverse engineering was done against as well, and checking this
# module's declarations against a different build's dex would answer a question
# nobody asked.
PINNED="$(sed -n "s/^ *name *: *'\(.*\)',/\1/p" build.gradle | head -1)"
[ -n "$PINNED" ] || { echo "no APK name in build.gradle" >&2; exit 1; }
APK=""
for candidate in "apk/$PINNED" "../stuff/$PINNED"; do
    [ -f "$candidate" ] && { APK="$candidate"; break; }
done
[ -n "$APK" ] || { echo "no APK to read their side out of; see README.md" >&2; exit 1; }

# ---- their declarations, out of the APK's own dex ---------------------------
echo "reading $(basename "$APK")…"
$APKTOOL d -f -r -o "$WORK/theirs" "$APK" >/dev/null
THEIRS="$(dirname "$(find "$WORK/theirs" -path "*/$PKG/SessionBindings.smali" | head -1)")"
[ -d "$THEIRS" ] || { echo "$APK has no $PKG in it" >&2; exit 1; }

# One line per member: kind, static-ness, name, descriptor. Bodies, and every
# modifier except `static` (which decides whether a native is handed a class or
# an object), are not linkage and are dropped.
smali_decls() {
    awk '
        /^\.method/ {
            st = ($0 ~ / static /) ? "S" : "-"
            line = $0
            sub(/^\.method[a-z ]*/, "", line)
            i = index(line, "(")
            print "M", st, substr(line, 1, i - 1), substr(line, i)
        }
        /^\.field/ {
            st = ($0 ~ / static /) ? "S" : "-"
            line = $0
            sub(/^\.field[a-z ]*/, "", line)
            sub(/ =.*$/, "", line)
            i = index(line, ":")
            print "F", st, substr(line, 1, i - 1), substr(line, i + 1)
        }
    ' "$@" | sort -u
}

# ---- ours, out of the class files -------------------------------------------
# javap prints every member's descriptor, which is the same string the core's
# GetMethodID is called with, so the two sides compare as text.
javap_decls() {
    javap -p -s "$@" | awk '
        /^(Compiled|[A-Za-z].*\{$|\}$)/ {
            if ($0 ~ /\{$/) { n = split($0, w, " "); simple = w[n - 1]; sub(/^.*\./, "", simple) }
            next
        }
        /^ +descriptor: / {
            if (pending != "") { print pending, $2 }
            pending = ""
            next
        }
        {
            line = $0
            st = (line ~ /(^| )static /) ? "S" : "-"
            sub(/^ +/, "", line)
            sub(/;$/, "", line)
            if (index(line, "(") > 0) {
                head = substr(line, 1, index(line, "(") - 1)
                n = split(head, w, " ")
                name = w[n]
                sub(/^.*\./, "", name)
                if (name == simple) { name = "<init>" }
                pending = "M " st " " name
            } else {
                n = split(line, w, " ")
                pending = "F " st " " w[n]
            }
        }
    ' | sort -u
}

status=0

# ---- (1) every native declared here is a symbol in the library --------------
SYMBOLS="$(readelf -W --dyn-syms "$SO" | awk '$4=="FUNC"{print $NF}')"
natives=0
for class_file in "$CLASSES"/*.class; do
    base="$(basename "$class_file" .class)"
    # A nested class carries its nesting into the symbol, as _00024. (An
    # underscore in a name would become _1; there are none here.)
    mangled_class="${base//$/_00024}"
    javap_decls "$class_file" > "$WORK/decls.txt"
    # The name is what is left of the declaration once the argument list is
    # cut off — not the last field, which for anything with arguments is the
    # last argument's type.
    while read -r name; do
        [ -n "$name" ] || continue
        desc="$(awk -v n="$name" '$1=="M" && $3==n {print $4}' "$WORK/decls.txt")"
        symbol="Java_com_realvnc_vncviewer_jni_${mangled_class}_${name}"
        natives=$((natives + 1))
        if grep -Fxq -- "$symbol" <<<"$SYMBOLS"; then
            printf 'ok       native %s.%s %s\n' "$base" "$name" "$desc"
        else
            printf 'MISSING  native %s.%s has no %s in %s\n' \
                "$base" "$name" "$symbol" "$(basename "$SO")"
            status=1
        fi
    done < <(javap -p "$class_file" | grep ' native ' | sed 's/(.*//' | awk '{print $NF}')
done
echo "         $natives natives"

# ---- (2) every callback the core resolves is named in the binary ------------
# A native that drifts is caught above, by its symbol. A *callback* that drifts
# is not: the core looks it up by string at run time, so the evidence is the
# string table. An interface whose name is not in there is one the core never
# resolves — this module has one, the sealed type over the form elements — and
# it is reported rather than checked.
STRINGS="$WORK/strings.txt"
strings -a -n 1 "$SO" | sort -u > "$STRINGS"
for class_file in "$CLASSES"/*.class; do
    javap -p "$class_file" | head -2 | grep -q 'interface ' || continue
    base="$(basename "$class_file" .class)"
    if ! grep -Fxq "com/realvnc/vncviewer/jni/${base}" "$STRINGS"; then
        printf 'ours     %s (no such class name in %s)\n' "$base" "$(basename "$SO")"
        continue
    fi
    javap_decls "$class_file" > "$WORK/decls.txt"
    while read -r kind st member desc; do
        [ "$kind" = 'M' ] || continue
        if grep -Fxq "$member" "$STRINGS" && grep -Fxq "$desc" "$STRINGS"; then
            printf 'ok       callback %s.%s %s\n' "$base" "$member" "$desc"
        else
            printf 'MISSING  callback %s.%s %s is not looked up by %s\n' \
                "$base" "$member" "$desc" "$(basename "$SO")"
            status=1
        fi
    done < "$WORK/decls.txt"
done

# ---- (3) where both sides declare the same name, they agree -----------------
for ours_file in src/main/java/$PKG/*.java; do
    name="$(basename "$ours_file" .java)"
    [ "$name" = package-info ] && continue

    ours_classes=()
    for f in "$CLASSES/$name.class" "$CLASSES/$name\$"*.class; do
        [ -f "$f" ] && ours_classes+=("$f")
    done
    [ ${#ours_classes[@]} -gt 0 ] || continue

    theirs_files=()
    for f in "$THEIRS/$name.smali" "$THEIRS/$name\$"*.smali; do
        [ -f "$f" ] && theirs_files+=("$f")
    done
    if [ ${#theirs_files[@]} -eq 0 ]; then
        printf 'ours     %s (no class of this name in theirs)\n' "$name"
        continue
    fi

    smali_decls "${theirs_files[@]}"  > "$WORK/t.txt"
    javap_decls "${ours_classes[@]}"  > "$WORK/o.txt"

    clash=""
    while read -r kind st member desc; do
        theirs_same_name="$(awk -v k="$kind" -v m="$member" '$1==k && $3==m' "$WORK/t.txt")"
        [ -n "$theirs_same_name" ] || continue
        if ! grep -Fxq -- "$kind $st $member $desc" <<<"$theirs_same_name"; then
            clash="${clash}           ${member}: here [${st}] ${desc}, theirs "
            clash="${clash}$(echo "$theirs_same_name" | awk '{printf "[%s] %s  ", $2, $4}')"$'\n'
        fi
    done < "$WORK/o.txt"

    if [ -n "$clash" ]; then
        printf 'MISMATCH %s\n%s' "$name" "$clash"
        status=1
    else
        printf 'ok       %s\n' "$name"
    fi
done

echo
if [ "$status" != 0 ]; then
    echo "FAIL: a declaration here does not match the library it binds to."
    echo "      A native with no symbol throws UnsatisfiedLinkError on first call;"
    echo "      a member whose shape has drifted throws NoSuchMethodError, later."
else
    echo "PASS: every native has its symbol, and every shared name has the same shape."
fi
exit "$status"
