#!/bin/sh
# Makes /etc/searxng/settings.yml (tmpfs) from the baked settings with a fresh random secret_key, then runs upstream's entrypoint,
# which finds the file and starts the server. The key lives only in that file and is new on every start.
set -eu
secret=$(head -c 48 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9')
[ "${#secret}" -ge 32 ] || { echo "could not make a random secret_key" >&2; exit 1; }
sed "s/@SECRET@/$secret/" /usr/local/searxng/askbar-settings.yml > /etc/searxng/settings.yml
exec /usr/local/searxng/entrypoint.sh
