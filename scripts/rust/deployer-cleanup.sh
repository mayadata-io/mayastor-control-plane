#!/usr/bin/env bash

SCRIPT_DIR="$(dirname "$0")"
export ROOT_DIR="$SCRIPT_DIR/../.."
SUDO=$(which sudo)

cleanup_ws_tmp() {
  # This contains tmp for container artifacts, example: pool disk images
  tmp_dir="$(realpath "$ROOT_DIR/.tmp")"

  for device in $(losetup -l -J | jq -r --arg tmp_dir $tmp_dir '.loopdevices[]|select(."back-file" | startswith($tmp_dir)) | .name'); do
    echo "Found stale loop device: $device"

    $SUDO $(which vgremove) -y --select="pv_name=$device" || :
    $SUDO losetup -d "$device"
  done

  $SUDO rm -rf "$tmp_dir"

  return 0
}

$SUDO nvme disconnect-all
"$ROOT_DIR"/target/debug/deployer stop

for c in $(docker ps -a --filter "label=io.composer.test.name" --format '{{.ID}}') ; do
  docker kill "$c"
  docker rm -v "$c"
done

for n in $(docker network ls --filter "label=io.composer.test.name" --format '{{.ID}}') ; do
  docker network rm "$n" || ( $SUDO systemctl restart docker && docker network rm "$n" )
done

cleanup_ws_tmp
