# 0024. Rejected token alert

Status: Proposed
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

Per-link tokens are written once by `bootstrap`. If a token that worked is later rejected, it may mean a fault or a compromise.

## Decision

When a caller's previously accepted token is rejected, the caller records the failure and sends an alert naming the link. Repeated invalid tokens at a receiver also send an alert. Alerts never include the token.

## Alternatives considered

- **Logging the failure only:** rejected, because it could go unnoticed.
- **Stopping the service automatically:** rejected, because no service has Docker access, and a fault could cause an outage.

## Consequences

The admin gets early warning of a broken or attacked link, and investigates and rotates the token as needed. Alert rules must limit repeats, so one fault doesn't flood alerts.
