# Plan: one image, two run modes

Status: proposed. The decision it needs is written out as an ADR in [Proposed ADR](#proposed-adr), not yet filed. Nothing here is decided until the owner accepts it and approves the rule changes below ([AGENTS.md](../AGENTS.md), "Stop and ask first"). This is not v1 scope ([ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md)).

## Why

Askbar publishes eight images, and a release signs, scans and pins all eight ([ADR 0018](adr/0018-releases-and-migrations.md)). One image makes that one tag, one digest, one SBOM, one scan, one signature and one pull, and shortens `images.yml` and `scripts/release-compose.sh`.

Hosts such as Unraid, Synology and one-click installers also run one container far better than a stack of eight. A second run mode serves them, as an opt-in with weaker isolation.

## The two modes

- **Mode A, separate containers (default, recommended).** `compose.yaml` runs the one image once per service, each with its own `command:`. Every network, volume, user ID, hardening setting ([ADR 0032](adr/0032-container-hardening.md)) and memory limit stays as today. The security model does not change.
- **Mode B, all-in-one (opt-in).** `compose.all-in-one.yaml` runs the same image as one container, with a supervisor as PID 1. It is a **reduced isolation tier**. The guide says so, and the admin site shows the run mode, the way a private or public host is a deployer choice ([ADR 0015](adr/0015-certificates.md)).

## The image

- **Base:** `debian:bookworm-slim`, as now. It holds every Rust binary, the Caddy binary from the existing `xcaddy` stage, and CLIProxyAPI copied from its pinned upstream image ([ADR 0037](adr/0037-askbar-cpa-image-and-supervisor.md)). CLIProxyAPI is MIT: `scripts/third-party-notices.images.md` and the image's licence labels must cover it.
- **Users:** every service user is made with the ids it has now: `askbar-admin` 10001, `askbar` (the server) 10002, `askbar-auditor` 10003, `askbar-gateway` (`model-gateway`) 10004, `askbar-alerter` 10005. `bootstrap`, Caddy and `cpa` run as root today (Caddy and `cpa` in group 10001), so they get no user in Mode A.
- **Directories:** every data and token directory is created and owned by its uid, with the modes the Dockerfile sets now. Docker seeds a new named volume from the image, and the auditor and alerter rely on that.
- **No `VOLUME`.** Otherwise every container would get an anonymous volume for every service's paths. Compose declares the volumes.
- **No `USER`, `ENTRYPOINT` or `HEALTHCHECK`.** Compose sets `user:`, `command:` or `entrypoint:`, and `healthcheck:` for each service.
- **`ENV`:** the names are distinct today, and only `ASKBAR_CPA_CONFIG_DIR` appears twice (admin and `cpa`, the same value). They still move to Compose, so a container sees only its own service's paths.
- **CA certificates:** the Rust services use `rustls` with built-in roots. Caddy takes its roots from the system today, from the official Alpine-based Caddy image, and CLIProxyAPI probably does too. The shared image needs `ca-certificates`. This is to be checked in step 1.1.

### What Mode A still shares

Each container holds every binary. A compromised `server` could run the admin or `model-gateway` binary. It would gain nothing: it has none of that service's volumes, tokens, networks or memory. The residual risk is that the server's container no longer proves "no admin code" by its contents, only by what it can reach. It goes in the [risk register](adr/0016-risk-register.md) as a new accepted row.

## Proposed ADR

The text below is the ADR to file if the owner accepts this plan. It is not filed yet: it has no number and is not in the [ADR index](adr/README.md). Once accepted, it is filed under the next free number with `Status: Accepted`, and the links in the rest of this plan point to it ([ADR 0001](adr/0001-decision-records-and-change-rules.md)).

Title: **One image, two run modes.** Source: owner request, 2026-11-13.

### Context

Askbar builds eight images (`server`, `bootstrap`, `auditor`, `model-gateway`, `alerter`, `admin`, `caddy`, `cpa`). Each needs its own tag, digest, SBOM, scan and signature, and a release file names all eight ([0018](adr/0018-releases-and-migrations.md)). The Rust binaries share one build stage and one base, so the split buys little except the rule that the server image holds no admin code ([0028](adr/0028-service-boundaries-and-config-delivery.md)).

Some hosts, such as Unraid, Synology and one-click installers, run one container well and a Compose stack of eight badly.

What keeps the services apart today is the container: its user ID, volumes, networks, capabilities and memory limit ([0003](adr/0003-v1-service-layout.md), [0032](adr/0032-container-hardening.md)). The image does not.

### Decision

1. **One image.** The build makes a single image, proposed name `askbar`, with every service in it: the Rust binaries, Caddy, and CLIProxyAPI (copied from its pinned upstream image, [0037](adr/0037-askbar-cpa-image-and-supervisor.md)). Every service user and data directory is created in it. The image has no `VOLUME`, `USER`, `ENTRYPOINT` or `HEALTHCHECK`; Compose sets each per service.
2. **Mode A, separate containers (the default).** `compose.yaml` runs the one image once per service, with its own `command:`, `user:` and `healthcheck:`. Every container name, network, volume, user ID, hardening setting and memory limit stays as it is. Each container runs only its own service's process.
3. **Mode B, all-in-one (opt-in).** A separate file, `compose.all-in-one.yaml`, runs the same image as one container, with a supervisor as PID 1 that starts each service as its own user and enforces the separation that Docker no longer does. It is a **reduced isolation tier**. The guide says so where it describes it, and the admin site shows the run mode. `compose.yaml` never starts it.
4. **Mode B rules.** The supervisor drops every capability from each service. Internal links use Unix sockets in per-link groups. Egress is limited by user ID. The settings and checks are [below](#phase-2-mode-b-all-in-one).
5. **One security model for the data.** A volume, token, secret or audit rule holds in both modes. Only the isolation tier differs.

### Alternatives considered

- **Eight images, as now:** the current state. It keeps the server image free of admin code, at the cost of eight of everything in the release, and gives appliance hosts nothing. Not chosen if the owner accepts this ADR; rejecting it keeps this.
- **One image, Mode A only:** a real option. It gives the release benefits and leaves the security model alone, but does nothing for appliance hosts.
- **Mode B only:** rejected. It would make the weaker tier the only one.
- **A supervisor from outside the project (s6-overlay):** open. It is a new dependency and needs its own licence check ([0026](adr/0026-dependency-license-check.md)). The plan recommends our own supervisor, following the `cpa-supervisor` pattern.
- **Per-process namespaces inside Mode B (bubblewrap, nsjail):** rejected unless the owner asks. They need user namespaces, which Docker's default seccomp profile blocks, so they mean loosening the outer container.
- **Giving a service the Docker socket to start the others:** never ([0005](adr/0005-no-controller-and-self-restart.md)).

### Consequences

- **Release:** one tag, digest, SBOM, scan, signature and pull. The image is larger, and a CVE in any component shows in every container's scan.
- **Mode A, honest cost:** every container holds every binary. A compromised `server` could run the admin or `model-gateway` binary, but gets none of that service's volumes, tokens, networks or memory. This residual risk goes in the risk register ([0016](adr/0016-risk-register.md)).
- **Mode B, what is lost:** network, mount and PID namespaces and per-service memory limits are replaced by firewall rules, Unix permissions and rlimits. Root in the container, or a kernel escape, reaches every service at once. The table is [below](#what-mode-b-loses).
- **Rules this conflicts with.** None changes until the owner accepts this ADR:
  - [0028](adr/0028-service-boundaries-and-config-delivery.md) rule 1 ("separate containers, images ... the server image contains no admin code") and the matching hard rule in [AGENTS.md](../AGENTS.md);
  - [0003](adr/0003-v1-service-layout.md), [0032](adr/0032-container-hardening.md), [0018](adr/0018-releases-and-migrations.md), [0037](adr/0037-askbar-cpa-image-and-supervisor.md), [0006](adr/0006-v1-outbound-access-and-alerts.md), [0016](adr/0016-risk-register.md) and [0038](adr/0038-names-and-new-components-without-a-name-check.md);
  - the component ADRs that name their own image.

  The amendments are listed [below](#adr-and-rule-changes).
- **Enforced by:** not enforced yet. Mode A adds checks to `scripts/check-isolation.sh`; Mode B adds `scripts/check-all-in-one.sh`.

## Phase 0: decide

0.1 **The owner accepts or rejects the [proposed ADR](#proposed-adr).** If accepted, it is filed under the next free number. Done when the filed ADR's status is Accepted and its index row matches.

0.2 **The owner approves the rule changes** in [ADR and rule changes](#adr-and-rule-changes), and the amendments are made, one commit each ([ADR 0001](adr/0001-decision-records-and-change-rules.md) rules 13 and 14). Done when `scripts/check-docs.sh` passes and no accepted doc still says the server image holds no admin code.

0.3 **The owner chooses** the open questions below that block phase 1: the image name and `cpa`.

## Phase 1: one image, Mode A

Each step ships alone. Every step brings its tests and docs.

1.1 **Add the Dockerfile target `askbar`** next to the old ones. Done when the image holds each binary, the user and directory listed above, the right owners, and no `VOLUME`, `USER`, `ENTRYPOINT` or `HEALTHCHECK` (a test reads `docker inspect`), and the Caddy and CLIProxyAPI binaries run in it (`caddy version`, CLIProxyAPI's own version flag). The CLIProxyAPI binary's libraries are checked here: if it is not static, it fails here and not in production.

1.2 **Switch `compose.yaml`.** Each service uses image `askbar` with `command:`, `user:`, `healthcheck:` and its `environment:`. Container names, networks, volumes, `x-hardening` and limits do not change. `compose.staging.yaml` follows. The old targets are removed. Done when `scripts/check-isolation.sh` passes with no check loosened, and `tests/staging-compose.test.sh` passes.

1.3 **Change what the old image split checked.** `scripts/check-isolation.sh` section 5 tests that the server image holds no admin binary; that can no longer be true. Replace it, and say so in the commit ([AGENTS.md](../AGENTS.md): do not loosen a test unless the contract changed). Add, for each container: its main process is its own service's binary, running as its uid, with `CapEff` as today. Done when each container fails the check if started with another service's command.

1.4 **Release.** `.github/workflows/images.yml` builds, scans and signs one image; `scripts/release-compose.sh`, `tests/release-compose.test.sh` and `scripts/third-party-notices.images.md` follow, and the image labels name CLIProxyAPI's MIT licence. Workflow changes need the owner's approval first. Done when a dry run publishes one image with an SBOM and one signature, `compose.release.yaml` names one digest, and `cargo deny` and the npm licence check pass.

1.5 **Migration and guide.** Volume names do not change: a data volume is never renamed. Image names do change, from `askbar-<service>` to `askbar`. The guide ("Using pre-built images") says to run `docker compose pull` and `docker compose up -d`, and names the old images to delete. Done when an install made from the last release upgrades in place with its data, and a person has tried it.

Before 1.0 nothing needs migrating ([ADR 0018](adr/0018-releases-and-migrations.md)), so Mode A is cheapest before 1.0. It is not on the v1 ship checklist. Do it before 1.0 only if the owner finds it cheap; otherwise it goes after 1.0 with the other plans ([plan-post-v1.md](plan-post-v1.md)).

## Phase 2: Mode B, all-in-one

After 1.0. Each item is a step with its check. The checks are in `scripts/check-all-in-one.sh` (step 2.8).

2.1 **A supervisor as PID 1.** Our own Rust supervisor, in the pattern of `cpa-supervisor` ([ADR 0037](adr/0037-askbar-cpa-image-and-supervisor.md)): no new dependency, no port, no token. The alternative is s6-overlay, a new dependency that needs its own licence check ([ADR 0026](adr/0026-dependency-license-check.md)). The owner chooses; this plan recommends our own.
   - It starts each service as its own uid and gid (`setgid`, then `setuid`), drops every capability from the child, and sets `no_new_privs`.
   - Order: `bootstrap` (one-shot), then the auditor, then `model-gateway`, the server, the alerter and the admin, then Caddy, then `cpa` if it is on.
   - A service that exits is restarted with backoff, so the admin's restart actions keep working ([ADR 0005](adr/0005-no-controller-and-self-restart.md)).
   - The supervisor keeps `SETUID` and `SETGID` for its life, because every restart needs them. It is the trusted base of this mode, so it stays small, opens nothing and parses nothing from a service.
   - Check: each process runs as its uid with `CapEff` 0 (Caddy keeps `NET_BIND_SERVICE`, as in Mode A), and the admin's restart of each service brings it back.

2.2 **Unix sockets for every internal link.** Each socket sits in a directory owned by a group shared by only the two services of that link, so a link's group mirrors today's per-link network. Per-link tokens ([ADR 0004](adr/0004-bootstrap-and-per-link-tokens.md)) stay as a second barrier. Services listen on TCP today (the admin and Caddy's admin API already use sockets), so each listener and caller in `askbar-service` gains a socket form. Caddy proxies to the server and the admin over sockets, and the admin's Caddy configuration names them. Check: the server's uid cannot open the admin's socket; no service listens on a TCP port but Caddy's 80 and 443.

2.3 **Egress by uid.** At start the supervisor installs `nftables` rules on `meta skuid`: only `model-gateway`, `cpa` and the alerter may leave the host, and Caddy for certificate requests and inbound. The server, auditor, admin and `bootstrap` get no route out ([ADR 0006](adr/0006-v1-outbound-access-and-alerts.md)).
   - The rules need `CAP_NET_ADMIN`. The supervisor drops it, and removes it from its bounding set, before it starts any service.
   - Docker allows `cap_add: [NET_ADMIN]`. It is an exception to [ADR 0032](adr/0032-container-hardening.md), recorded there.
   - If the rules cannot be installed, for instance on a kernel with no `nftables` (some NAS kernels), no service starts. The mode fails closed.
   - Check: from the server's uid, a connection to the internet fails; from the gateway's, it works.

2.4 **Container settings.** `read_only`, `cap_drop: [ALL]`, then `cap_add` of only `SETUID`, `SETGID`, `NET_ADMIN`, `NET_BIND_SERVICE` and `CHOWN` (`bootstrap`'s exception, [ADR 0032](adr/0032-container-hardening.md) rule 6). `no-new-privileges:true`. One `pids_limit` and one `mem_limit` for the container. `tmpfs` at `/tmp`, with a mode 0700 directory for each service inside it. After start, no service process holds a capability except Caddy's `NET_BIND_SERVICE`, as in Mode A; the supervisor keeps `SETUID` and `SETGID` (step 2.1). Check: `docker inspect` shows exactly this set.

2.5 **Files and secrets.** Every data and token directory is mode 0700 and owned by its service's uid. No secret is in an environment variable (an existing rule). `/proc/<pid>/environ` and another uid's memory stay unreadable, because the uids differ and no process has `CAP_SYS_PTRACE`. Mount `/proc` with `hidepid=2` if the runtime allows it; remounting needs `CAP_SYS_ADMIN`, which this mode does not grant, so expect to record why not. Check: the server's uid cannot read another service's directory, environ or memory.

2.6 **Limits without cgroups.** The supervisor sets `RLIMIT_AS` and `RLIMIT_NPROC` for each child, and raises the OOM score of every service but the auditor, so a runaway service is killed first. The auditor's fail-closed behaviour ([ADR 0010](adr/0010-auditor.md), [ADR 0051](adr/0051-auditor.md)) still blocks model calls if it is down. Check: a service that allocates without limit is killed and restarted, and the auditor keeps running.

2.7 **Token rotation and `bootstrap`.** `bootstrap` runs first as a one-shot child with `CHOWN` as its only capability, then `CHOWN` is dropped. Rotation from the host runs it again in the running container (`docker compose exec`), and the admin then restarts the affected services ([ADR 0005](adr/0005-no-controller-and-self-restart.md)).

2.8 **`scripts/check-all-in-one.sh`** (needs Docker), in the style of `scripts/check-isolation.sh`. Done when it shows:
   - each process runs as its uid, with no capabilities;
   - the server's uid cannot connect out, open the admin's socket or read another service's directory;
   - only the allowed uids reach the internet;
   - stopping the auditor stops chat replies, and starting it restores them.

2.9 **`compose.all-in-one.yaml`, the guide and the admin site.** The guide gets a section that says plainly what this mode gives up. The admin site shows the run mode in a label it sets from a variable that holds a mode name, not a secret, and writes it with `textContent`. Done when a person has run a fresh install and a paired client on it.

2.10 **From Mode B to Mode A.** The volume names and paths are the same, so a move is: stop, switch the compose file, `docker compose up -d`. If the uids differ in any way, restore from a backup ([ADR 0013](adr/0013-backups.md)). Done when both ways are tried with data in place, and the guide says which to use.

### What Mode B loses

Hardening narrows the gap and does not close it.

| Guarantee | Mode A | Mode B |
| --- | --- | --- |
| Network separation | One kernel network namespace per container, one `internal` network per link | One namespace; `nftables` rules by uid and Unix sockets in per-link groups |
| Mount separation | Each container sees only its own volumes | One filesystem; Unix owner and mode 0700 |
| PID separation | A service sees only its own processes | One PID space, `hidepid=2` if allowed; no ptrace by uid |
| Memory limits | A cgroup limit for each service | One cgroup limit, plus `RLIMIT_AS`, `RLIMIT_NPROC` and OOM scores |
| Capabilities | `CapEff` 0 per container, with Caddy's and `bootstrap`'s exceptions | `CapEff` 0 per service process; the supervisor keeps `SETUID` and `SETGID` |
| Egress | No route for the server, auditor and admin (internal networks) | The same, by `nftables`; fails closed if they cannot be installed |
| Root in the container, or a kernel or local privilege escalation | Reaches one service | Reaches every service's data, the provider keys in `model-gateway`'s memory, the admin database and the audit store at once |

The last row is the main loss. The new [risk register](adr/0016-risk-register.md) row for Mode B reads: "Root in the all-in-one container, or a kernel escape, reaches every service. Accepted for the reduced tier only; the deployer chose it for a host that cannot run separate containers. Mitigation: the settings above and `scripts/check-all-in-one.sh`; use Mode A where you can."

## Phase 3: more architectures

3.1 **Multi-arch (amd64 and arm64)** for NAS hosts, if the images do not already have it. `images.yml` builds one platform today. Done when both platforms build, scan and sign, and the Rust build uses a native arm64 runner and not emulation. Check that the pinned CLIProxyAPI image has an arm64 build.

## ADR and rule changes

The owner must approve these. The files are not edited in this pull request. Each is made after the [proposed ADR](#proposed-adr) is accepted and filed, one commit each ([ADR 0001](adr/0001-decision-records-and-change-rules.md) rules 13 and 14); a decision change needs the owner's approval, and a change that only describes how a component is built goes in with the code.

| Rule or ADR | Kind | Change |
| --- | --- | --- |
| [AGENTS.md](../AGENTS.md), hard rule "The admin stays in its own container and image. No admin code in the server image." | Decision | Reword: the admin stays in its own container, user and volumes in Mode A; the image is shared. Mode B shares the container and is the reduced tier |
| [0028](adr/0028-service-boundaries-and-config-delivery.md) rule 1 | Decision | Separate containers, user IDs and volumes stay (Mode A); the image is shared; the server image's "no admin code" goes. Mode B shares the container |
| [0003](adr/0003-v1-service-layout.md) | Decision | Services are one image run as separate containers, or in Mode B as one |
| [0032](adr/0032-container-hardening.md) | Decision | Mode B's exceptions: `NET_ADMIN`, `SETUID`, `SETGID`, `NET_BIND_SERVICE`, `CHOWN` together, and the supervisor |
| [0018](adr/0018-releases-and-migrations.md) and `images.yml` | Decision | One image instead of eight, and what the release file names |
| [0037](adr/0037-askbar-cpa-image-and-supervisor.md) | Decision | No separate `askbar-cpa` image; CLIProxyAPI is copied into the one image |
| [0006](adr/0006-v1-outbound-access-and-alerts.md) | Decision | Mode B limits egress by `nftables` and uid, not by network |
| [0016](adr/0016-risk-register.md) | Register | Two accepted rows: every binary in every Mode A container; root in the Mode B container |
| [0038](adr/0038-names-and-new-components-without-a-name-check.md) and the [glossary](architecture.md#glossary) | Description | The image name (`askbar`) and the term "run mode"; the Served by column |
| [0039](adr/0039-server.md), [0040](adr/0040-admin.md), [0041](adr/0041-caddy.md), [0042](adr/0042-cpa.md), [0049](adr/0049-bootstrap.md), [0051](adr/0051-auditor.md), [0054](adr/0054-model-gateway.md), [0055](adr/0055-alerter.md) | Description | Each names its own image; with the code, each names `askbar` and its `command:` |
| [architecture.md](architecture.md), [SECURITY.md](../SECURITY.md), [guide.md](guide.md), [third-party notices](../scripts/third-party-notices.images.md) | Description | Image names, "Why each boundary exists", and the Mode B section |

## Open questions

- **The image name.** `askbar` is the proposal.
- **The supervisor:** our own Rust one (recommended) or s6-overlay.
- **When Mode B ships:** after 1.0 (recommended) or before.
- **Should `cpa` stay a separate optional image?** It would keep a third-party binary out of the main image, at the price of two images. Today `cpa` is optional only at run time.
- **`hidepid` availability** on the target runtimes ([step 2.5](#phase-2-mode-b-all-in-one)).
- **How the supervisor installs `nftables` rules:** the `nft` program in the image (one more Debian package in every container, to the same licence rule) or a netlink library (a new dependency).
- **Users for Caddy and `cpa` in Mode B.** They run as root with group 10001 in Mode A. Mode B needs their own uids; CLIProxyAPI writes under `$HOME` and has not been tried as another user.
- **The routing plan's model files.** [plan-tiered-routing.md](plan-tiered-routing.md) adds a classifier to `model-gateway`; in one image it would load into every container.

## Not planned

- **Per-process namespaces** (bubblewrap, nsjail) in Mode B. They need user namespaces, which Docker's default seccomp profile blocks, so they mean loosening the outer container. Rejected unless the owner asks.
- **The Docker socket for any service.** Never ([ADR 0005](adr/0005-no-controller-and-self-restart.md)).
