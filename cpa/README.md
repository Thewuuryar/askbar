# CLIProxyAPI (profile `cpa`)

CLIProxyAPI turns a Claude, ChatGPT/Codex or Gemini **subscription login** into an
OpenAI-compatible endpoint. Askbar's server talks to it over the compose network; its API
port is never published to the host.

```bash
docker compose --profile cpa up -d            # creates the config with random secrets on first run
docker compose run --rm cpa-init show         # prints the API key to use in Askbar
```

## One-time login

Pick the provider you have a subscription for. The command prints a URL; open it in the
browser **on the machine running Docker**. (Ignore the "SSH tunnel" hint it prints.) After you
approve, the browser is redirected to `localhost`, which reaches the container because
`--service-ports` publishes the callback port on `127.0.0.1` for this command only.

```bash
# Claude
docker compose --profile cpa run --rm --service-ports cpa-login ./CLIProxyAPI -config /cpa-config/config.yaml -claude-login -no-browser
# ChatGPT / Codex
docker compose --profile cpa run --rm --service-ports cpa-login ./CLIProxyAPI -config /cpa-config/config.yaml -codex-login -no-browser
```

Other providers: `docker compose run --rm cpa-login ./CLIProxyAPI -h`. The login is stored in the
`cpa-auth` volume and survives restarts. CLIProxyAPI picks up new logins without a restart.

## Use it from Askbar

Admin page, Providers, **Add CLIProxyAPI**: the base URL is `http://cpa:8317/v1`; paste the API
key from `cpa-init show`; for the model, use a name from CLIProxyAPI's list (the server image has no curl, so ask from a throwaway container):

```bash
docker run --rm --network askbar_default python:3.12-slim python -c "import urllib.request as u;r=u.Request('http://cpa:8317/v1/models',headers={'Authorization':'Bearer YOUR_KEY'});print(u.urlopen(r).read().decode())"
```

Press **Test** on the provider card, then ask a question from the desktop client.

The config and the API key live in the `cpa-config` volume, not in this repository.
