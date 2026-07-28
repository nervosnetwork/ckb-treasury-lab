#!/bin/sh
set -e
pid=$(pgrep -f "ckb run") || {
	echo "ckb run (devnet) is not running" >&2
	exit 1
}
echo "attaching dtrace to ckb (pid $pid)" >&2
exec sudo dtrace -s "$(dirname "$0")/probe.sh" -p "$pid"
