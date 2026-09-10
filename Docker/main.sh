#!/bin/sh
set -eu

CONFIG_FILE=/root/config.json

if [ ! -f "$CONFIG_FILE" ]; then
    cp /app/config.json "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
fi

cd /root
exec /app/TGBot_RSS "$@"

