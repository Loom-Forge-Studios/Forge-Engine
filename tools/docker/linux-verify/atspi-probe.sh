#!/usr/bin/env bash
# M2-22 on Linux (WP-U12): observe the AT-SPI tree of `ui_gallery` the way a screen reader
# does. Run inside the verify container, under its X server:
#
#   just verify-linux bash tools/docker/linux-verify/atspi-probe.sh
#
# A private D-Bus session (dbus-run-session), the accessibility bus and registry
# (at-spi2-core's at-spi-bus-launcher), accessibility switched on for the session
# (`atspi_probe enable`, as a screen reader starting does), then ui_gallery, then the
# client walks the application's tree over the bus (`atspi_probe walk`). Exit 0 only if the
# widgets the probe expects were found with their roles and names.
set -euo pipefail
cargo build --locked -p forge-ui --example ui_gallery --example atspi_probe
BIN="${CARGO_TARGET_DIR:-target}/debug/examples"
echo "atspi-probe: $(dpkg-query -W -f='${Package} ${Version}\n' at-spi2-core dbus 2>/dev/null | tr '\n' ';' )"
exec dbus-run-session -- bash -s "$BIN" <<'SESSION'
set -euo pipefail
BIN="$1"
/usr/libexec/at-spi-bus-launcher --launch-immediately &
LAUNCHER=$!
for _ in $(seq 1 50); do
    if "$BIN/atspi_probe" enable; then break; fi
    sleep 0.2
done
"$BIN/ui_gallery" --exit-after 40 > /tmp/ui_gallery.log 2>&1 &
GALLERY=$!
STATUS=0
"$BIN/atspi_probe" walk "" 60 || STATUS=$?
kill "$GALLERY" 2>/dev/null || true
wait "$GALLERY" 2>/dev/null || true
kill "$LAUNCHER" 2>/dev/null || true
echo "atspi-probe: ui_gallery log:"
sed 's/^/  /' /tmp/ui_gallery.log || true
exit "$STATUS"
SESSION
