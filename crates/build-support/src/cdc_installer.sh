{
# CDC bootstrap v1. The complete compound command is parsed before any binary request.
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
stage=
reader=
watchdog=
cleanup() {
    result=$?
    trap - EXIT HUP INT TERM
    if [ -n "$reader" ]; then kill "$reader" 2>/dev/null || :; wait "$reader" 2>/dev/null || :; fi
    if [ -n "$watchdog" ]; then kill "$watchdog" 2>/dev/null || :; wait "$watchdog" 2>/dev/null || :; fi
    if [ -n "$stage" ]; then rm -rf "$stage"; fi
    [ "$result" -eq 0 ] || printf 'CDC installation failed (%s).\n' "$result" >&2
    exit "$result"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
for tool in uname mkdir mktemp dd wc shasum chmod mv rm sleep nohup; do command -v "$tool" >/dev/null || exit 1; done
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || { printf 'This image supports native Apple Silicon macOS only.\n' >&2; exit 1; }
set -- /dev/cu.usbmodemP*3
[ "$#" -eq 1 ] && [ -c "$1" ] || exit 1
port=$1
printf 'M1 arm64\n' >&3
IFS=' ' read -r -t 5 version arch size block digest extra <&3
[ "$version" = C1 ] && [ "$arch" = arm64 ] && [ "$block" = 16384 ] && [ -z "$extra" ] || exit 1
case "$size" in ''|*[!0-9]*|0|0*) exit 1;; esac
[ "${#size}" -le 7 ] && [ "$size" -le 4194304 ] || exit 1
case "$digest" in *[!0-9a-f]*) exit 1;; esac
[ "${#digest}" -eq 64 ] || exit 1
dest=${PICO_CDC_INSTALL_DIR:-$HOME/pico-agent}
[ ! -L "$dest" ] || exit 1
mkdir -p "$dest"
[ -d "$dest" ] && [ -O "$dest" ] && [ ! -L "$dest/HOSTAGNT" ] || exit 1
[ ! -e "$dest/HOSTAGNT" ] || [ -f "$dest/HOSTAGNT" ] || exit 1
stage=$(mktemp -d "$dest/.cdc.XXXXXX")
parent=$$
(
    # Standalone exec prevents saved shell redirection descriptors leaking to the watchdog.
    exec </dev/null 3>&-
    timer=
    trap '[ -z "$timer" ] || kill "$timer" 2>/dev/null || :; wait "$timer" 2>/dev/null || :; exit 0' HUP INT TERM
    sleep 120 & timer=$!
    printf ready >"$stage/watchdog.ready"
    wait "$timer"
    kill -TERM "$parent" 2>/dev/null || :
) & watchdog=$!
setup=0
while [ ! -f "$stage/watchdog.ready" ]; do
    [ "$setup" -lt 500 ] || exit 1
    sleep 0.01
    setup=$((setup+1))
done
mode=byte
if dd if=/dev/null iflag=fullblock count=0 >/dev/null 2>&1; then mode=full; fi
left=$size
index=0
: >"$stage/HOSTAGNT"
printf 'Downloading %s bytes over USB CDC...\n' "$size"
while [ "$left" -gt 0 ]; do
    take=$block
    [ "$left" -ge "$block" ] || take=$left
    printf 'G1 %s\n' "$index" >&3
    if [ "$mode" = full ]; then
        dd iflag=fullblock bs="$take" count=1 <&3 >>"$stage/HOSTAGNT" 2>/dev/null &
    else
        dd bs=1 count="$take" <&3 >>"$stage/HOSTAGNT" 2>/dev/null &
    fi
    reader=$!
    if wait "$reader"; then reader=; else result=$?; reader=; exit "$result"; fi
    left=$((left-take))
    index=$((index+1))
    [ "$(wc -c <"$stage/HOSTAGNT")" -eq "$((size-left))" ] || exit 1
done
set -- $(shasum -a 256 "$stage/HOSTAGNT")
[ "$1" = "$digest" ] || exit 1
chmod 700 "$stage/HOSTAGNT"
mv -f "$stage/HOSTAGNT" "$dest/HOSTAGNT"
printf 'V1\n' >&3
IFS= read -r -t 5 response <&3
[ "$response" = OK1 ] || exit 1
kill "$watchdog" 2>/dev/null || :
wait "$watchdog" 2>/dev/null || :
watchdog=
rm -rf "$stage"
stage=
printf 'Verified and installed %s; starting agent.\n' "$dest/HOSTAGNT"
exec </dev/null 3>&-
# Ignore HUP before forking: the child must inherit this disposition before nohup starts.
trap '' HUP
nohup "$dest/HOSTAGNT" --port "$port" </dev/null >/dev/null 2>&1 &
exit 0
}
