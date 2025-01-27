#!/usr/bin/env bash

SCRIPT_DIR="$(dirname "$0")"
export ROOT_DIR="$SCRIPT_DIR/.."
SUDO=$(which sudo)

set -eu

mkdir -p "$ROOT_DIR/ci-report"
cd "$ROOT_DIR/ci-report"

journalctl --since=-6h -o short-precise > journalctl.txt
journalctl -k -b0 -o short-precise > dmesg.txt
lsblk -tfa > lsblk.txt
$SUDO nvme list -v > nvme.txt
$SUDO nvme list-subsys -v >> nvme.txt
cat /proc/meminfo > meminfo.txt

tar -czvf ci-report.tar.gz ./*.txt ./*.xml
