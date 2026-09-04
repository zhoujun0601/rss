#!/bin/bash
set -eu

CONFIG_FILE=/root/config.json

# Keep the mounted configuration file, but synchronize it with the current
# container environment on every start.
if [ ! -f "$CONFIG_FILE" ]; then
    cp /app/config.json "$CONFIG_FILE"
fi

if ! jq empty "$CONFIG_FILE" >/dev/null 2>&1; then
    echo "Invalid config.json detected; restoring the bundled template" >&2
    cp /app/config.json "$CONFIG_FILE"
fi

cp -f /app/TGBot_RSS /root/TGBot_RSS
cd /root

BotToken=${BotToken:-}
ADMINIDS=${ADMINIDS:-0}
Cycletime=${Cycletime:-1}
Debug=${Debug:-false}
ProxyURL=${ProxyURL:-}
Pushinfo=${Pushinfo:-}
export TZ="${TZ:-Asia/Shanghai}"

case "$ADMINIDS" in
    ''|*[!0-9-]*) echo "ADMINIDS must be an integer" >&2; exit 1 ;;
esac
case "$Cycletime" in
    ''|*[!0-9-]*) echo "Cycletime must be an integer" >&2; exit 1 ;;
esac
case "$Debug" in
    true|false) ;;
    *) echo "Debug must be true or false" >&2; exit 1 ;;
esac

tmp_file=$(mktemp "${CONFIG_FILE}.tmp.XXXXXX")
trap 'rm -f "$tmp_file"' EXIT
jq \
    --arg BotToken "$BotToken" \
    --argjson ADMINIDS "$ADMINIDS" \
    --argjson Cycletime "$Cycletime" \
    --argjson Debug "$Debug" \
    --arg ProxyURL "$ProxyURL" \
    --arg Pushinfo "$Pushinfo" \
    '.BotToken = $BotToken
     | .ADMINIDS = $ADMINIDS
     | .Cycletime = $Cycletime
     | .Debug = $Debug
     | .ProxyURL = $ProxyURL
     | .Pushinfo = $Pushinfo' \
    "$CONFIG_FILE" > "$tmp_file"
mv -f "$tmp_file" "$CONFIG_FILE"
chmod 600 "$CONFIG_FILE"

exec ./TGBot_RSS
