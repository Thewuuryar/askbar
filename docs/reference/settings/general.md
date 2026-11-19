---
id: settings/general
title: General
summary: What every reply starts from (the system prompt, the reply time limit, the history sent with each question), what people are told, and the display defaults.
keywords: [general, system prompt, reply time limit, history, display, privacy notice]
settings: [general]
---

# General

What every reply starts from: the system prompt, defaults and limits. The audit retention, the time sealed content stays open and the kill switch are on the [Security](security.md) page.

## System prompt

Sent with every message. It is never shown to clients.

## Reply time limit

A reply still running after this long is stopped and the text so far is kept. A message that waits in the queue is timed from when it starts.

## History sent with each question

A new question goes to the model with this many earlier questions and their replies. 0 sends the whole conversation (the default). A shorter history uses fewer tokens; the model does not see what is left out. A question that got no reply is never sent again.

## What people are told

The privacy notice is shown in the app after pairing and again whenever its wording or the audit retention (set on the Security page) changes. The AI warning is shown above the input box. Both are on by default.

## Display defaults

Clients use these unless their own client.yaml overrides them.
