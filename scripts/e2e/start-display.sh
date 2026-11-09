#!/bin/bash
# Start a private Xvfb display and tauri-driver for driving the launcher
# (scripts/e2e/real-launch.py).
#
# Usage: start-display.sh <new run dir> [existing HOME dir]
#
# The run dir must not exist; it is created with:
#   home/   scratch HOME (the app keeps its data in $HOME/.launcher), unless an
#           existing HOME dir is given, as for an upgrade test where the next
#           version opens the data the previous one left
#   shim/   first on PATH; holds an xdg-open stand-in that logs its arguments
#   xdg/    XDG_RUNTIME_DIR (mode 700)
# and on success holds display.txt (the display number), wd-url.txt (the
# WebDriver URL), and xvfb.pid / driver.pid (process groups) for stop.sh.
#
# Java for the administrator comes from JAVA_HOME, which must be a
# JavaFX-enabled JDK. tauri-driver and WebKitWebDriver (webkit2gtk-driver) must be on PATH.
set -euo pipefail

R=${1:?usage: start-display.sh <new run dir> [existing HOME dir]}
[ -n "${JAVA_HOME:-}" ] || { echo "JAVA_HOME is not set" >&2; exit 1; }
if [ -n "${2:-}" ]; then
  [ -d "$2" ] || { echo "HOME dir $2 does not exist" >&2; exit 1; }
  APP_HOME=$(cd "$2" && pwd)
fi
mkdir "$R"
R=$(cd "$R" && pwd)
if [ -z "${APP_HOME:-}" ]; then
  mkdir "$R/home"
  APP_HOME=$R/home
fi
echo "$APP_HOME" > "$R/home-dir.txt"
mkdir "$R/shim" "$R/xdg"
chmod 700 "$R/xdg"

cat > "$R/shim/xdg-open" <<EOF
#!/bin/sh
echo "\$(date -u +%FT%TZ) xdg-open \$*" >> "$R/xdg-open.log"
EOF
chmod +x "$R/shim/xdg-open"

# Xvfb picks a free display number and writes it to fd 3.
setsid Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3> "$R/display.txt" \
  > "$R/xvfb.log" 2>&1 < /dev/null &
echo $! > "$R/xvfb.pid"
for _ in $(seq 1 50); do
  [ -s "$R/display.txt" ] && break
  sleep 0.1
done
N=$(tr -d '[:space:]' < "$R/display.txt")
[ -n "$N" ] || { echo "Xvfb did not report a display" >&2; exit 1; }

read -r P Q < <(python3 -I -c '
import socket
s1, s2 = socket.socket(), socket.socket()
s1.bind(("127.0.0.1", 0)); s2.bind(("127.0.0.1", 0))
print(s1.getsockname()[1], s2.getsockname()[1])')

setsid env HOME="$APP_HOME" PATH="$R/shim:$PATH" DISPLAY=":$N" XDG_RUNTIME_DIR="$R/xdg" \
  GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 JAVA_HOME="$JAVA_HOME" \
  dbus-run-session -- tauri-driver --port "$P" --native-port "$Q" \
  > "$R/driver.log" 2>&1 < /dev/null &
echo $! > "$R/driver.pid"

for _ in $(seq 1 100); do
  if curl -s "http://127.0.0.1:$P/status" > /dev/null 2>&1; then
    echo "http://127.0.0.1:$P" > "$R/wd-url.txt"
    echo "display :$N, driver http://127.0.0.1:$P, run dir $R"
    exit 0
  fi
  sleep 0.1
done
echo "tauri-driver did not answer on port $P; see $R/driver.log" >&2
exit 1
