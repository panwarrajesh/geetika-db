#!/usr/bin/env bash
# Copy RajeshDB backups off this machine. Run from cron (see docs/DEPLOYMENT.md).
#   RAJESHDB_OFFSITE_DEST="user@backuphost:/backups/rajeshdb"   -> rsync over SSH
#   RAJESHDB_OFFSITE_DEST="rclone:myremote:rajeshdb-backups"    -> rclone (S3, B2, Drive, ... configure with `rclone config`;
#                                                                  use an rclone "crypt" remote to encrypt)
# Nothing is deleted at the destination; old backups pile up there until YOU prune them.
set -euo pipefail

SRC="${RAJESHDB_BACKUP_SRC:-/srv/rajeshdb-data/backups}"
DEST="${RAJESHDB_OFFSITE_DEST:?set RAJESHDB_OFFSITE_DEST (see the comments at the top of this script)}"

[ -d "$SRC" ] || { echo "backup folder not found: $SRC" >&2; exit 1; }

if [[ "$DEST" == rclone:* ]]; then
  command -v rclone >/dev/null || { echo "rclone is not installed" >&2; exit 1; }
  rclone copy "$SRC" "${DEST#rclone:}" --exclude '*.partial/**'
else
  command -v rsync >/dev/null || { echo "rsync is not installed" >&2; exit 1; }
  rsync -a --partial --exclude '*.partial/' "$SRC"/ "$DEST"/
fi
echo "$(date -Is) offsite copy finished: $SRC -> $DEST"
