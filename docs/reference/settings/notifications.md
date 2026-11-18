---
id: settings/notifications
title: Notifications
summary: Where Askbar sends alerts (ntfy and email), and how to send a test alert.
keywords: [notifications, alerts, ntfy, email, test alert, banners, alerter]
settings: [notifications]
---

# Notifications

Where Askbar sends an alert when something needs attention, and how to test it. The alerter delivers the alerts; the guide explains its setup and what sends alerts: [The alerter](../../guide.md#the-alerter). The kinds of alert are listed in [Alert kinds](../audit/alert-kinds.md).

## ntfy

Alerts go to ntfy.sh or your own ntfy server. Every value is secret: it is kept encrypted, never shown again, and leaves the admin only for the alerter.

## Email

Mail goes from the alerter to your mail server. A server on the internet needs STARTTLS or TLS, because a password is not sent without TLS.

## Test alert

Sends a test alert to every channel that is set up.

## Page-wide banners

A banner across every page for these conditions. Critical ones are always shown.
