#!/bin/bash
# Start an Open Integration Engine from its release tarball, as a plain process, for the
# real-launch test (scripts/e2e/real-launch.py).
#
# Usage: start-engine.sh <oie_unix_*.tar.gz> <new folder> <admin password>
#
# Unpacks into <folder>/oie, sets the initial admin password and two free ports on 127.0.0.1
# in mirth.properties, and starts oieserver with JAVA_HOME (a Java 17 JDK). Waits up to 5
# minutes for "successfully started" in the engine's log. On success <folder> holds
# engine-url.txt (https://127.0.0.1:<port>) and engine.pid (the engine's process group) for
# stop.sh. The HTTPS port is the one the engine also writes into the administrator's JNLP, so
# the address the launcher uses is the one the administrator logs in to.
set -euo pipefail
usage="usage: start-engine.sh <oie_unix_*.tar.gz> <new folder> <admin password>"
TARBALL=${1:?$usage}
DIR=${2:?$usage}
PASSWORD=${3:?$usage}
[ -n "${JAVA_HOME:-}" ] || { echo "JAVA_HOME is not set" >&2; exit 1; }

mkdir "$DIR"
DIR=$(cd "$DIR" && pwd)
tar xzf "$TARBALL" -C "$DIR"
OIE=$DIR/oie
P=$OIE/conf/mirth.properties
[ -f "$P" ] && [ -x "$OIE/oieserver" ] || { echo "$TARBALL doesn't unpack to oie/conf/mirth.properties and oie/oieserver" >&2; exit 1; }

read -r HTTPS HTTP < <(python3 -I -c '
import socket
a, b = socket.socket(), socket.socket()
a.bind(("127.0.0.1", 0)); b.bind(("127.0.0.1", 0))
print(a.getsockname()[1], b.getsockname()[1])')

# The shipped lines carry trailing spaces, so match any whitespace and check each took.
set_prop() {  # $1 key, $2 value
  local key=${1//./\\.}
  sed -i -E "s/^${key}[[:space:]]*=.*\$/$1 = $2/" "$P"
  grep -qxF "$1 = $2" "$P" || { echo "could not set $1 in $P" >&2; exit 1; }
}
set_prop https.port "$HTTPS"
set_prop http.port "$HTTP"
set_prop https.host 127.0.0.1
set_prop http.host 127.0.0.1
set_prop server.initialadminpassword "$PASSWORD"

# setsid makes the engine the leader of its own process group, so stop.sh can end it whole.
(cd "$OIE" && INSTALL4J_JAVA_HOME="$JAVA_HOME" exec setsid ./oieserver > "$DIR/oieserver.out" 2>&1 < /dev/null) &
echo $! > "$DIR/engine.pid"
LOG=$OIE/logs/mirth.log
for _ in $(seq 1 150); do
  if [ -f "$LOG" ] && grep -q 'successfully started' "$LOG"; then
    echo "https://127.0.0.1:$HTTPS" > "$DIR/engine-url.txt"
    grep 'successfully started' "$LOG"
    echo "engine at https://127.0.0.1:$HTTPS (folder $DIR)"
    exit 0
  fi
  if ! kill -0 "$(cat "$DIR/engine.pid")" 2> /dev/null; then
    echo "oieserver exited; see $DIR/oieserver.out and $LOG" >&2; exit 1
  fi
  sleep 2
done
echo "the engine didn't start within 5 minutes; see $LOG" >&2
exit 1
