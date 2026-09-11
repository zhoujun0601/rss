#!/bin/sh
set -eu

DATA_DIR=/data
CONFIG_FILE="$DATA_DIR/config.json"

if [ ! -f "$CONFIG_FILE" ]; then
    cp /app/config.json "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
fi

chown tgbot:tgbot "$DATA_DIR" "$CONFIG_FILE"
for runtime_file in "$DATA_DIR"/tgbot.db* "$DATA_DIR"/bot.log; do
    if [ -e "$runtime_file" ]; then
        chown tgbot:tgbot "$runtime_file"
    fi
done

cd "$DATA_DIR"
exec gosu tgbot /app/TGBot_RSS "$@"
