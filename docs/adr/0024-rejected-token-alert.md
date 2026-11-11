# 0024. Rejected token alert

Status: Accepted
Date: 2026-11-04
Source: recommendation accepted after the clean copy

## Context

Per-link tokens are written once by `bootstrap`. If a token that worked is later rejected, it may mean a fault or a compromise.

## Decision

When a caller's previously accepted token is rejected, the caller records the failure and sends an alert naming the link. The admin sends `rejected_token` for every link it calls ([0055](0055-alerter.md)). Repeat limiting is the alerter's (one delivery for each derived key in 60 minutes). Repeated invalid tokens at a receiver also send an alert. Alerts never include the token.

## Alternatives considered

- **Logging the failure only:** rejected, because it could go unnoticed.
- **Stopping the service automatically:** rejected, because no service has Docker access, and a fault could cause an outage.

## Consequences

The admin gets early warning of a broken or attacked link, and investigates and rotates the token as needed. Alert rules must limit repeats, so one fault doesn't flood alerts.

## Amendments
- 2026-11-11: the admin sends `rejected_token` (#70, D1 and D7, a decision change for D7): the sender is named, and repeat limiting is the alerter's, keyed by the kind and the link, not by a key the sender supplies.
