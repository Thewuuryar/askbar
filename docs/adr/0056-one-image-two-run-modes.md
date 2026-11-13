# 0056. One image, two run modes

Status: Proposed
Date: 2026-11-13
Source: owner request, 2026-11-13

## Context

Askbar builds eight images (`server`, `bootstrap`, `auditor`, `model-gateway`, `alerter`, `admin`, `caddy`, `cpa`). Each needs its own tag, digest, SBOM, scan and signature, and a release file names all eight ([0018](0018-releases-and-migrations.md)). The Rust binaries share one build stage and one base, so the split buys little except the rule that the server image holds no admin code ([0028](0028-service-boundaries-and-config-delivery.md)).

Some hosts, such as Unraid, Synology and one-click installers, run one container well and a Compose stack of eight badly.

What keeps the services apart today is the container: its user ID, volumes, networks, capabilities and memory limit ([0003](0003-v1-service-layout.md), [0032](0032-container-hardening.md)). The image does not.

## Decision

1. **One image.** The build makes a single image, proposed name `askbar`, with every service in it: the Rust binaries, Caddy, and CLIProxyAPI (copied from its pinned upstream image, [0037](0037-askbar-cpa-image-and-supervisor.md)). Every service user and data directory is created in it. The image has no `VOLUME`, `USER`, `ENTRYPOINT` or `HEALTHCHECK`; Compose sets each per service.
2. **Mode A, separate containers (the default).** `compose.yaml` runs the one image once per service, with its own `command:`, `user:` and `healthcheck:`. Every container name, network, volume, user ID, hardening setting and memory limit stays as it is. Each container runs only its own service's process.
3. **Mode B, all-in-one (opt-in).** A separate file, `compose.all-in-one.yaml`, runs the same image as one container, with a supervisor as PID 1 that starts each service as its own user and enforces the separation that Docker no longer does. It is a **reduced isolation tier**. The guide says so where it describes it, and the admin site shows the run mode. `compose.yaml` never starts it.
4. **Mode B rules.** The supervisor drops every capability from each service. Internal links use Unix sockets in per-link groups. Egress is limited by user ID. The settings and checks are in the [plan](../plan-single-image.md#phase-2-mode-b-all-in-one).
5. **One security model for the data.** A volume, token, secret or audit rule holds in both modes. Only the isolation tier differs.

## Alternatives considered

- **Eight images, as now:** the current state. It keeps the server image free of admin code, at the cost of eight of everything in the release, and gives appliance hosts nothing. Not chosen if the owner accepts this ADR; rejecting it keeps this.
- **One image, Mode A only:** a real option. It gives the release benefits and leaves the security model alone, but does nothing for appliance hosts.
- **Mode B only:** rejected. It would make the weaker tier the only one.
- **A supervisor from outside the project (s6-overlay):** open. It is a new dependency and needs its own licence check ([0026](0026-dependency-license-check.md)). The plan recommends our own supervisor, following the `cpa-supervisor` pattern.
- **Per-process namespaces inside Mode B (bubblewrap, nsjail):** rejected unless the owner asks. They need user namespaces, which Docker's default seccomp profile blocks, so they mean loosening the outer container.
- **Giving a service the Docker socket to start the others:** never ([0005](0005-no-controller-and-self-restart.md)).

## Consequences

- **Release:** one tag, digest, SBOM, scan, signature and pull. The image is larger, and a CVE in any component shows in every container's scan.
- **Mode A, honest cost:** every container holds every binary. A compromised `server` could run the admin or `model-gateway` binary, but gets none of that service's volumes, tokens, networks or memory. This residual risk goes in the risk register ([0016](0016-risk-register.md)).
- **Mode B, what is lost:** network, mount and PID namespaces and per-service memory limits are replaced by firewall rules, Unix permissions and rlimits. Root in the container, or a kernel escape, reaches every service at once. The table is in the [plan](../plan-single-image.md#what-mode-b-loses).
- **Rules this conflicts with.** None changes until the owner accepts this ADR:
  - [0028](0028-service-boundaries-and-config-delivery.md) rule 1 ("separate containers, images ... the server image contains no admin code") and the matching hard rule in [AGENTS.md](../../AGENTS.md);
  - [0003](0003-v1-service-layout.md), [0032](0032-container-hardening.md), [0018](0018-releases-and-migrations.md), [0037](0037-askbar-cpa-image-and-supervisor.md), [0006](0006-v1-outbound-access-and-alerts.md), [0016](0016-risk-register.md) and [0038](0038-names-and-new-components-without-a-name-check.md);
  - the component ADRs that name their own image.

  The amendments are listed in the [plan](../plan-single-image.md#adr-and-rule-changes).
- **Enforced by:** not enforced yet. Mode A adds checks to `scripts/check-isolation.sh`; Mode B adds `scripts/check-all-in-one.sh`.

## Amendments

