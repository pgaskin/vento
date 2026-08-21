#!/bin/bash
# Builds the website into build/. Requires curl and ImageMagick on the host.
set -euo pipefail
cd "$(dirname "$0")"

die() {
    echo "$@" >&2
    exit 1
}

rm -rf build
mkdir build

shopt -s nullglob
changelogs=../app/fastlane/metadata/android/en-US/changelogs
files=("$changelogs"/*.txt)
shopt -u nullglob
test ${#files[@]} -gt 0 || die "no changelogs in $changelogs"

versions=$(printf '%s\n' "${files[@]}" | sed 's|.*/||; s|\.txt$||' | sort -rn)
version=$(printf '          <span>Version %s</span>' "$(printf '%s\n' "$versions" | head -n1)")

whatsnew=$(
    set -e
    indent='          '
    n=0
    for v in $versions; do
        open=''
        if [ $n -lt 2 ]; then
            open=' open'
        fi
        if [ $n -eq 8 ]; then
            printf '%s<details class="more">\n' "$indent"
            printf '%s  <summary>More versions</summary>\n' "$indent"
            indent="$indent  "
        fi
        printf '%s<details class="release"%s>\n' "$indent" "$open"
        printf '%s  <summary>Version %s</summary>\n' "$indent" "$v"
        printf '%s  <ul>\n' "$indent"
        sed -e 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g' "$changelogs/$v.txt" | sed -n "s|^\* \(.*\)\$|$indent    <li>\1</li>|p"
        printf '%s  </ul>\n' "$indent"
        printf '%s</details>\n' "$indent"
        n=$((n + 1))
    done
    if [ $n -gt 8 ]; then
        printf '%s</details>\n' "${indent%  }"
    fi
)

whatsnew="$whatsnew" version="$version" awk '
    /<!-- CHANGELOG -->/ { printf "%s\n", ENVIRON["whatsnew"]; changelog = 1; next }
    /<!-- VERSION -->/ { printf "%s\n", ENVIRON["version"]; ver = 1; next }
    { print }
    END {
        if (!changelog) { print "no CHANGELOG placeholder in index.html" >"/dev/stderr"; exit 1 }
        if (!ver) { print "no VERSION placeholder in index.html" >"/dev/stderr"; exit 1 }
    }
' index.html >build/index.html
echo "build/index.html"

ua='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36'

font() {
    css=$(set -e; curl -fsS -A "$ua" "https://fonts.googleapis.com/css2?family=$1&display=swap")
    url=$(set -e; awk '/\/\* latin \*\//{f=1} f && /src: url\(/{sub(/.*url\(/, ""); sub(/\).*/, ""); print; exit}' <<<"$css")
    test -n "$url" || die "no latin subset in the google fonts css for $1"
    curl -fsS -o "$2" "$url"
    echo "$2"
}
font 'Roboto:wght@400..700' build/roboto.woff2

icon=../app/fastlane/metadata/android/en-US/images/icon.png
shots=../app/fastlane/metadata/android/en-US/images/phoneScreenshots
test -f "$icon" || die "missing $icon"

# imagemagick 7 calls it magick, 6 only has convert
im=$(command -v magick || command -v convert) || die "imagemagick is required"

cp "$icon" build/icon.png
echo "build/icon.png"

for n in 1 4 3 5 2 9; do
    test -f "$shots/$n.png" || die "missing $shots/$n.png"
    "$im" "$shots/$n.png" -resize 540x1200 -quality 88 -sampling-factor 1x1 -strip "build/shot-$n.jpg"
    echo "build/shot-$n.jpg"
done
