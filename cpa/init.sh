#!/bin/sh
# Creates CLIProxyAPI's config on first run, with random secrets, inside the cpa-config volume.
# The secrets are generated here and never stored in this repository.
#   init.sh          create the config if it does not exist yet
#   init.sh show     print the API key to enter in Askbar's admin page (Providers > Add CLIProxyAPI)
set -eu
CONFIG=/cpa-config/config.yaml
KEYFILE=/cpa-config/askbar-api-key
rand() { tr -dc 'A-Za-z0-9' < /dev/urandom | head -c "$1"; }

if [ "${1:-}" = "show" ]; then
  [ -f "$KEYFILE" ] || { echo "No CLIProxyAPI config yet. Run: docker compose --profile cpa up -d" >&2; exit 1; }
  cat "$KEYFILE"; echo
  exit 0
fi

if [ -f "$CONFIG" ]; then
  echo "CLIProxyAPI config already exists; leaving it alone."
  exit 0
fi

API_KEY="sk-cpa-$(rand 40)"
umask 077
printf '%s' "$API_KEY" > "$KEYFILE"
cat > "$CONFIG" <<YAML
config-version: 8
server:
  host: ""
  port: 8317
oauth:
  auth-dir: "/root/.cli-proxy-api"
access:
  api-keys:
    - "$API_KEY"
management:
  # Reachable only on the compose network: the API port is not published to the host.
  allow-remote: true
  secret-key: "$(rand 40)"
YAML
echo "Created $CONFIG"
echo "Your CLIProxyAPI API key (enter it as the API key in Askbar's admin page):"
echo "$API_KEY"
