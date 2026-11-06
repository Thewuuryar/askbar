# CLIProxyAPI

This page describes the code on `main`. The accepted v1 target differs: see [docs/architecture.md](../../docs/architecture.md] and the [ADRs](../../docs/adr/README.md). Where they disagree, build to the ADR.

CLIProxyAPI turns a Claude or ChatGPT/Codex **subscription login** into an OpenAI-compatible endpoint.
Askbar's server talks to it over the compose network; its API port is never published to the host.

There is nothing to run in a terminal. In the admin site, **Services** tab:

1. **Connect Claude** or **Connect ChatGPT**, open the address it shows and approve.
2. The browser ends on a `http://localhost...` address that usually does not load. Paste that whole address
   into the box and press **Finish**.
3. The **CPA** provider appears on the Providers tab. Pick its model from the dropdown and press **Test**.

The admin makes CLIProxyAPI's `config.yaml`, its API key and its control secret itself (in the `cpa-config`
volume and the admin's secrets), and uses the key to set up the CPA provider. The logins live in the `cpa-auth`
volume and survive restarts. **Disconnect** on the same panel removes an account.

Other CLIProxyAPI providers (Gemini and so on) are not connected from the admin site yet; for those, add a
provider by hand.
