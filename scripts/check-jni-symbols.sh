#!/bin/bash
# Does every `native` method we declare exist in the library we ship?
#
# `scripts/check-jni-abi.sh` asks this of somebody else's binary; this
# asks it of ours, and the failure mode is the same one: a native that is not
# where Java looks for it does not fail to compile. It fails the first time
# that method is called, as an UnsatisfiedLinkError, which for something like
# a thumbnail read might be the first time the home screen is shown after a
# session.
#
# A library binds its natives one of two ways, and each is asked its own
# question:
#
#   exported    by mangled name, `Java_<class>_<method>`: RealVNC's traffic
#               counter, the one native left outside the generic layer. Every
#               native the class declares must be a dynamic symbol.
#   registered  by `RegisterNatives` from the library's `JNI_OnLoad`: every
#               other backend, which is one JNI layer (backends/platform/android)
#               registered on each module's generated natives class: a cdylib
#               of the protocol crate for the four in Rust, and for the three
#               in C the same layer linked into the library Meson makes, where
#               `JNI_OnLoad` is the only export. There is no symbol to look
#               for, so what is checked is what registration is made of:
#                 - the library exports `JNI_OnLoad` and names the class it
#                   registers on;
#                 - the table it registers, read out of the library itself
#                   (each `JNINativeMethod` is a name, a descriptor and a
#                   function, three relocated pointers in a row, so the pairs
#                   come back exactly as `RegisterNatives` is handed them), is
#                   the compiled class's natives, name for name and descriptor
#                   for descriptor, both ways: one Java declares and the table
#                   lacks is an UnsatisfiedLinkError when called, and one the
#                   table has and Java does not declare fails the whole
#                   registration when the library loads;
#                 - the callbacks the layer looks up on `NativeBackend` (the
#                   `on…` methods and the two `knownHosts…` it keeps its pins
#                   through, each a `jni_str!`/`jni_sig!` pair in its source)
#                   are the ones `NativeBackend` declares, both ways, and each
#                   pair's name and descriptor are strings in the library, which
#                   is as far as a binary shows a lookup made in code.
#               (That each Rust function matches its descriptor is checked when
#               it compiles, by `jni::native_method!`.)
#
# One library per protocol module, which is what the loop below is: each is
# asked about its own declarations, and a module deleted is a line deleted.
#
#   scripts/check-jni-symbols.sh            # builds if needed
#   NOBUILD=1 scripts/check-jni-symbols.sh  # against whatever is already there
set -euo pipefail
cd "$(dirname "$0")/.."   # the build is at the repository root

# module : the class declaring the natives : the library it loads : how it is
# built : how it binds
MODULES=(
    "rfb:net/pgaskin/remotedesktop/backend/rfb/RfbNatives:remotedesktop_rfb:cargo:registered"
    "libvnc:net/pgaskin/remotedesktop/backend/libvnc/LibVncNatives:remotedesktop_libvnc:meson:registered"
    "tigervnc:net/pgaskin/remotedesktop/backend/tigervnc/TigerVncNatives:remotedesktop_tigervnc:meson:registered"
    "ironrdp:net/pgaskin/remotedesktop/backend/ironrdp/IronRdpNatives:remotedesktop_ironrdp:cargo:registered"
    "freerdp:net/pgaskin/remotedesktop/backend/freerdp/FreeRdpNatives:remotedesktop_freerdp:meson:registered"
    "realvnc:net/pgaskin/remotedesktop/backend/realvnc/RealVncTraffic:remotedesktop_realvnc:meson:exported"
    "rustdesk:net/pgaskin/remotedesktop/backend/rustdesk/RustDeskNatives:remotedesktop_rustdesk:cargo:registered"
    "spice:net/pgaskin/remotedesktop/backend/spice/SpiceNatives:remotedesktop_spice:cargo:registered"
)

# The class every registered library calls back, compiled once in :backends.
EVENTS=net/pgaskin/remotedesktop/backend/NativeBackend

if [ "${NOBUILD:-0}" != 1 ]; then
    tasks=(":backends:compileDebugJavaWithJavac")
    for entry in "${MODULES[@]}"; do
        IFS=: read -r module _ _ built bound <<<"$entry"
        case "$built" in
            cargo) tasks+=(":backends:${module}:cargoBuildDebugNative") ;;
            meson) tasks+=(":backends:${module}:mesonBuildDebug") ;;
        esac
        # A generated class is only on disk once it has been compiled.
        [ "$bound" = registered ] && tasks+=(":backends:${module}:compileDebugJavaWithJavac")
    done
    ./gradlew --quiet "${tasks[@]}"
fi

# A compiled class, newest first for the reason the library is (below).
class_file() {
    find "$1" -path '*/javac/*' -path "*/classes/$2.class" -printf '%T@ %p\n' 2>/dev/null \
        | sort -rn | head -1 | cut -d' ' -f2-
}

# `name descriptor` for each method of a class file that matches `$2` (a
# grep -E on javap's declaration line): javap prints the descriptor on the
# line after each declaration, which is the same string RegisterNatives and
# GetMethodID are called with, so the two sides compare as text.
methods() {
    javap -p -s "$1" | awk -v want="$2" '
        /^ +descriptor: / { if (name != "") print name, $2; name = ""; next }
        /\(/ && $0 ~ want {
            line = $0; sub(/\(.*/, "", line); n = split(line, w, " "); name = w[n]; next
        }
        { name = "" }
    '
}

# `name descriptor` of each `JNINativeMethod` in a library: three relocated
# pointers in a row (name, descriptor, function) whose first two point at
# strings shaped like a Java name and a method descriptor. The strings are in
# `.rodata` and the pointers are RELATIVE relocations with their targets as
# addends, which is how both the NDK's clang and rustc's lld lay out a static
# table in a shared library; the same table can be there twice (one copy per
# use in a debug build), hence the `sort -u`.
registered() {
    local so="$1"
    {
        readelf -W -S "$so" | awk '{ sub(/^ *\[ *[0-9]+\] */, "") } $1 == ".rodata" { print "SEC", $3 }'
        readelf -W -p .rodata "$so" | LC_ALL=C awk 'match($0, /^ *\[ *[0-9a-f]+\]  /) {
            off = substr($0, 1, RLENGTH); gsub(/[][ ]/, "", off); print "STR", off, substr($0, RLENGTH + 1) }'
        readelf -W -r "$so" | awk '$3 ~ /_RELATIVE$/ { print "REL", $1, $4 }'
    } | LC_ALL=C awk '
        function hex(s,   i, v) {
            v = 0; s = tolower(s)
            for (i = 1; i <= length(s); i++) v = v * 16 + index("0123456789abcdef", substr(s, i, 1)) - 1
            return v
        }
        # One parameter or return type of a descriptor.
        BEGIN { t = "(\\[*([ZBCSIJFD]|L[A-Za-z0-9_/$]+;))" }
        $1 == "SEC" { base = hex($2); next }
        $1 == "STR" { s = $0; sub(/^STR [0-9a-f]+ /, "", s); str[base + hex($2)] = s; next }
        $1 == "REL" { rel[hex($2)] = hex($3); order[++n] = hex($2); next }
        END {
            for (i = 1; i <= n; i++) {
                o = order[i]
                if (!((o + 8) in rel) || !((o + 16) in rel)) continue
                name = str[rel[o]]; desc = str[rel[o + 8]]
                if (name ~ /^[A-Za-z_][A-Za-z0-9_]*$/ && desc ~ ("^\\(" t "*\\)(V|" t ")$")) print name, desc
            }
        }' | sort -u
}

# The JNI layer every registered library is built with, and the callbacks it
# looks up on NativeBackend: each `jni_str!("name"), jni_sig!("descriptor")`
# pair in it, the file read as one line since a call can span several.
LAYER=backends/platform/android/src/backend.rs
looked_up="$(tr '\n' ' ' <"$LAYER" \
    | grep -oE 'jni_str!\("[A-Za-z]+"\), *jni_sig!\("[^"]+"\)' \
    | sed -E 's/jni_str!\("([^"]+)"\), *jni_sig!\("([^"]+)"\)/\1 \2/' | sort -u)"
if [ -z "$looked_up" ]; then
    echo "no callback lookups found in $LAYER" >&2
    exit 1
fi

fail=0
for entry in "${MODULES[@]}"; do
    IFS=: read -r module class lib built bound <<<"$entry"

    case "$built" in
        cargo) root="backends/modules/$module/build/generated/rustNativeLibs" ;;
        meson) root="backends/modules/$module/build/generated/mesonLibs" ;;
    esac
    # The newest, not the first: a C module keeps a directory per variant, so
    # `find | head -1` can hand back a library from an older build, and the
    # check would then report on a binary that is not shipped, in both
    # directions (a MISSING for a method that had been there for a stage, for
    # example).
    so="$(find "$root" -name "lib${lib}.so" -printf '%T@ %p\n' 2>/dev/null \
            | sort -rn | head -1 | cut -d' ' -f2-)"
    if [ -z "$so" ]; then
        echo "no lib${lib}.so — run ./gradlew :backends:${module}:assembleDebug" >&2
        exit 1
    fi
    echo "== ${module}: ${so#*/build/} (${bound})"

    # The host's readelf: these are ordinary ELF dynamic symbols and no cross
    # toolchain is needed to read them.
    symbols="$(readelf -W --dyn-syms "$so" | awk '$4=="FUNC"{print $NF}')"

    if [ "$bound" = exported ]; then
        # A module's Java is the files themselves, with no package directories.
        java="backends/modules/$module/java/$(basename "$class").java"
        prefix="Java_$(dirname "$class" | tr / _)_$(basename "$class")"
        while read -r method; do
            [ -n "$method" ] || continue
            if grep -qx "${prefix}_${method}" <<<"$symbols"; then
                printf 'ok       %s.%s\n' "$(basename "$class")" "$method"
            else
                printf 'MISSING  %s.%s (%s)\n' "$(basename "$class")" "$method" "${prefix}_${method}"
                fail=1
            fi
        done < <(grep -E 'static +native' "$java" | grep -oE '\bnative[A-Z][A-Za-z0-9]*')
        continue
    fi

    # ---- registered -----------------------------------------------------------
    natives_class="$(class_file "backends/modules/$module/build" "$class")"
    events_class="$(class_file backends/build "$EVENTS")"
    if [ -z "$natives_class" ] || [ -z "$events_class" ]; then
        echo "no compiled $(basename "$class") or NativeBackend — run without NOBUILD=1" >&2
        exit 1
    fi
    strings="$(strings -a -n 1 "$so" | sort -u)"
    has() { grep -Fxq -- "$1" <<<"$strings"; }

    if grep -qx JNI_OnLoad <<<"$symbols"; then
        printf 'ok       JNI_OnLoad\n'
    else
        printf 'MISSING  JNI_OnLoad: nothing would register the natives\n'
        fail=1
    fi
    if has "$class"; then
        printf 'ok       registers on %s\n' "$class"
    else
        printf 'MISSING  registers on another class than %s\n' "$class"
        fail=1
    fi

    declared="$(methods "$natives_class" ' native ' | sort -u)"
    table="$(registered "$so")"
    # An empty side would make the check pass without checking anything.
    if [ -z "$declared" ]; then
        printf 'MISSING  %s declares no natives\n' "$(basename "$class")"
        fail=1
    fi
    if [ -z "$table" ]; then
        printf 'MISSING  no JNINativeMethod table found in %s\n' "$(basename "$so")"
        fail=1
    fi
    while read -r name desc; do
        [ -n "$name" ] || continue
        if grep -Fxq -- "$name $desc" <<<"$table"; then
            printf 'ok       native %s %s\n' "$name" "$desc"
        else
            printf 'MISSING  native %s %s is not in what %s registers\n' "$name" "$desc" "$(basename "$so")"
            fail=1
        fi
    done <<<"$declared"
    while read -r name desc; do
        [ -n "$name" ] || continue
        if ! grep -Fxq -- "$name $desc" <<<"$declared"; then
            printf 'EXTRA    native %s %s is registered but %s does not declare it\n' \
                "$name" "$desc" "$(basename "$class")"
            fail=1
        fi
    done <<<"$table"

    java_callbacks="$(methods "$events_class" ' (on|knownHosts)[A-Z][A-Za-z]*[(]' | sort -u)"
    if [ -z "$java_callbacks" ]; then
        printf 'MISSING  NativeBackend declares no callbacks\n'
        fail=1
    fi
    while read -r name desc; do
        [ -n "$name" ] || continue
        if ! grep -Fxq -- "$name $desc" <<<"$looked_up"; then
            printf 'MISSING  callback NativeBackend.%s %s is not looked up by %s\n' "$name" "$desc" "$LAYER"
            fail=1
        elif has "$name" && has "$desc"; then
            printf 'ok       callback NativeBackend.%s %s\n' "$name" "$desc"
        else
            printf 'MISSING  callback NativeBackend.%s %s is not in %s (built from an older %s?)\n' \
                "$name" "$desc" "$(basename "$so")" "$LAYER"
            fail=1
        fi
    done <<<"$java_callbacks"
    while read -r name desc; do
        [ -n "$name" ] || continue
        if ! grep -Fxq -- "$name $desc" <<<"$java_callbacks"; then
            printf 'EXTRA    callback %s %s is looked up by %s but NativeBackend does not declare it\n' \
                "$name" "$desc" "$LAYER"
            fail=1
        fi
    done <<<"$looked_up"
done

if [ "$fail" = 0 ]; then
    echo "PASS: each library registers exactly the natives its class declares (name and" \
        "descriptor, read from its table); the layer looks up exactly the callbacks" \
        "NativeBackend declares, each name and descriptor present in every library;" \
        "RealVNC's exported natives are symbols"
else
    echo "FAIL: see above" >&2
    exit 1
fi
