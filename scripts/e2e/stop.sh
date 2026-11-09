#!/bin/bash
# Stop what the e2e scripts started: each pid file names the leader of a process group
# (start-engine.sh, start-display.sh), and the whole group is ended. Deletes nothing.
# Usage: stop.sh <pid file>...
set -uo pipefail
for f in "$@"; do
  [ -s "$f" ] || { echo "$f: no pid"; continue; }
  pid=$(cat "$f")
  if kill -0 "$pid" 2> /dev/null; then
    kill -TERM -- "-$pid" 2> /dev/null || kill -TERM "$pid"
    for _ in $(seq 1 30); do kill -0 "$pid" 2> /dev/null || break; sleep 1; done
    if kill -0 "$pid" 2> /dev/null; then kill -KILL -- "-$pid" 2> /dev/null; echo "killed $f ($pid)"; else echo "stopped $f ($pid)"; fi
  else
    echo "$f ($pid) was not running"
  fi
done
