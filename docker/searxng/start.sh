#!/bin/sh
# Binds the web server to the one address the alias `searxng-host` gives (its address on mcp-searxng) so it answers nowhere else,
# not on egress-searxng; fails closed if the alias does not resolve. Then makes /etc/searxng/settings.yml (tmpfs) from the baked
# settings with a fresh random secret_key, and runs upstream's entrypoint, which finds the file and starts the server. The key
# lives only in that file and is new on every start.
set -eu
ip=
i=0
while [ "$i" -lt 30 ]; do
  ip=$(/usr/local/searxng/.venv/bin/python -c 'import socket; print(socket.gethostbyname("searxng-host"))' 2>/dev/null) || ip=
  case "$ip" in [0-9]*.[0-9]*.[0-9]*.[0-9]*) break;; esac
  ip=; i=$((i + 1)); sleep 1
done
[ -n "$ip" ] || { echo "searxng-host does not resolve: not starting" >&2; exit 1; }
export GRANIAN_HOST="$ip"
secret=$(head -c 48 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9')
[ "${#secret}" -ge 32 ] || { echo "could not make a random secret_key" >&2; exit 1; }
sed "s/@SECRET@/$secret/" /usr/local/searxng/askbar-settings.yml > /etc/searxng/settings.yml
exec /usr/local/searxng/entrypoint.sh
