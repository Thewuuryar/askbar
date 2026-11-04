# Plan: harden the containers

Status: Proposed, for review. Nothing here is built or tested yet.

## Why

The owner's priority is a compromised server, then a compromised admin, and keeping every path a model or a bug could use to break containment short. [ADR 0003](adr/0003-admin-and-server-isolation.md) depends on the first item below. Only the controller is hardened in `compose.yaml` today (`read_only`, `cap_drop: [ALL]`, `no-new-privileges`).

## Proposals

Each is small enough to ship alone. Order is by value for effort.

1. **Harden `server` and `admin`.** Set `read_only: true`, `cap_drop: [ALL]`, `security_opt: [no-new-privileges:true]`, a `pids_limit`, a memory limit and a `tmpfs` for `/tmp`. The writable paths are the volumes they already mount. Dropping `NET_RAW` and the other capabilities is what stops a compromised container forging a source address, which ADR 0003's address check relies on. Check by running the stack and the end-to-end tests; add checks to `scripts/check-isolation.sh` that each service has these settings.
2. **Harden `caddy` and `cpa` where they allow it.** Caddy binds port 443 and so needs either `NET_BIND_SERVICE` or the `net.ipv4.ip_unprivileged_port_start` setting; keep that one capability and drop the rest. `cpa` may need a writable config path ([plan-admin-managed-setup.md](plan-admin-managed-setup.md) notes it rewrites its own config). Test each; do not claim a setting works until it has run.
3. **Remove `host.docker.internal:host-gateway` from `server`.** It lets the server reach services on the host. Keep it only in a development override if the mock model needs it.
4. **Give the admin a fixed address on `server-api`**, as the server has, so the server can check where a claim comes from.
5. **Limit what the server can reach.** The server sits on `edge`, which has a full route out. A compromised server could send conversations anywhere. Docker has no built-in egress filter. Options: a forward proxy that allows only the configured provider hosts, or host firewall rules. Needs a decision on which, and on how the allowed list follows the admin's provider settings.
6. **Treat the server's replies as untrusted in the admin.** Check size and shape of every response the admin reads from the server API, and keep rendering them with `textContent`.
7. **Model gateway (future).** Move provider keys and outbound access into one small container, so the server holds no keys and has no route out. This fits the vocabulary rule in [ADR 0002](adr/0002-one-name-per-role-one-host.md): it becomes a named component when it gets its own container. Settle it before the executor in [ADR 0007](adr/0007-conversation-only-and-tool-gating.md).

## Open questions

- Which egress approach (item 5), and who maintains the allowed list?
- Do any services need a writable root filesystem (item 2)?
- Should hardening settings be one shared block in `compose.yaml` (a YAML anchor) so a new service starts hardened?
