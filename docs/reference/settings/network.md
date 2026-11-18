---
id: settings/network
title: Network
summary: The host name Askbar is reached at, its HTTPS certificate, and which addresses may open the admin site.
keywords: [network, host name, certificate, HTTPS, allowed addresses, networks, Caddy]
settings: [network]
---

# Network

The host name, its certificate, and which addresses may open this site. The guide covers using your own hostname and its certificate: [HTTPS with your own hostname](../../guide.md#https-with-your-own-hostname).

## Host name

Where the apps and this site are reached. Turn on HTTPS to serve through the Caddy front end. Enter a plain name such as askbar.example.com: no https://, port or path.

## Certificate

Browsers warn until Let's Encrypt issues a certificate. Prove the host name is yours with a DNS challenge through Cloudflare (works for a private address) or with HTTP-01, which needs public port 80 on this machine. Try the staging CA first: Let's Encrypt rate-limits failed attempts.

## Allowed addresses and networks

Limit who can open the admin site over HTTPS. Leave out the address you are using now and you lock yourself out. With the limit off, anyone who can reach the site may try to sign in.
