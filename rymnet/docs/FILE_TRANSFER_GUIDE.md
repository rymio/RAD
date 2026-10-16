# Rymnet File Transfer Guide

Send a file from one Rymnet account to another: sealed to the recipient
before it ever leaves your machine, delivered as mail with an
attachment. No blockchain, no upload server, no size-limited pastebin —
it reuses the same content-addressed chunk store `.rym` sites live in
and the same sealed-mail delivery every other private message in
Rymnet already uses.

## How it actually works (read this before you rely on it)

There is no presence system in Rymnet — no way for your node to ask
"where is this other person connected right now." File transfer
doesn't try to answer that question, the same way mail doesn't:

1. Your file's bytes are sealed (encrypted) to the recipient's mailbox
   key — the same public key their account already publishes for mail.
   Nobody but the recipient can ever decrypt them, including every peer
   that relays or stores the sealed copy along the way.
2. The sealed bytes are hashed (BLAKE3) and stored content-addressed,
   then pushed to every peer *you* have configured.
3. A small mail message — file name, size, and the hash needed to find
   and unseal it, nothing else — is sent to the recipient the normal
   way.
4. The recipient's client polls *their own* configured peers, finds the
   mail message, sees the attachment, and fetches the sealed chunk from
   whichever peer has it, unseals it locally, and saves it.

**This is near-real-time, not instant**, and it only works if your peer
network and the recipient's peer network overlap somewhere — the exact
same tradeoff mail already makes. If delivery seems to hang, check that
you actually have peers configured (`peer connect <ip>` in `rymsh`, or
see the Getting Started guide).

**The recipient has to have logged in at least once** while connected
to a shared peer, so their mailbox key is published somewhere you can
find it. If you get "no published mailbox key," that's why.

## From rymsh (terminal)

```text
rymsh:/home$ login yourname yourpassword
rymsh:yourname@/home$ sendfile notes.pdf alice
Looking up alice's mailbox key...
Pushing sealed file chunk to peers...
Chunk delivered to 2/2 peer(s).
Sent to alice: delivered to 2/2 peer(s).
```

`sendfile <path> <user>` reads `<path>` from your own virtual computer
(the same sandbox every other `rymsh` file command uses — if the file
is on your real disk, use `mc` first to copy it in). It seals the file,
pushes the chunk, and sends the pointer as a mail message with the
subject `File: notes.pdf`.

Receiving:

```text
rymsh:alice@/home$ email
Fetching inbox from 2 peer(s)...

Inbox: 1 message(s)
  #   From             Subject                              Date
  1   yourname         File: notes.pdf                       2026-07-13 09:14

email> 1

From:     yourname
To:       alice
Subject:  File: notes.pdf
Date:     2026-07-13 09:14
Verified: yes (signed by yourname's registered key)
Attachment: notes.pdf (482112 bytes, application/pdf)

email message [1]> s
Save as (path in your vfs) [notes.pdf]:
saved to notes.pdf
```

At the message prompt: `r` reply, `f` forward, `s` save the attachment,
`q` back to inbox. Leaving the save path blank uses the original
filename, saved into your current vfs location.

## From rymwin (desktop)

**Sending** — open **Terminal** from the Start menu:

```text
rymterm:yourname@/$ sendfile notes.pdf alice
Sending notes.pdf to alice...
Sent to alice: delivered to 2/2 peer(s).
```

Same `sendfile <path> <user>` command, same vfs-sandboxed path
resolution, running in the background so the terminal stays responsive
while the network calls are in flight.

**Receiving** — open **Mail**, select the message. If it has an
attachment, you'll see its name/size/type below the Verified line, plus
a *Save as* field and button. Leave the field blank to use the original
filename, or type a different vfs path, then click **Save**.

## What gets encrypted, and what doesn't

Sealed (only the recipient can read it): the file's actual bytes.

Not sealed, visible to relaying peers: the file's *name*, *size*, and
*MIME type* (these ride inside the mail message the same way a
subject line does — mail bodies are sealed too, but metadata a peer
needs to route/quota mail by is not). If a filename itself is
sensitive, rename the file before sending it.

## Limits

- Single-file transfers only — no folders, no batching. Send multiple
  files as multiple `sendfile` calls.
- Each node operator sets a `[files]` quota in their own `config.toml`
  (`quota_mb`, how much sealed file data a peer will relay for you
  total, and `max_file_size_mb`, the cap on any one transfer) — the
  defaults are generous (500&nbsp;MB / 100&nbsp;MB) but a large file can
  still be rejected by a particular peer. Try a different peer, or ask
  your node operator to raise the limit.
- No resume/chunked upload — a transfer is sealed and pushed as one
  blob. Fine for documents and images; not built for multi-gigabyte
  files yet.
- Forwarding a message doesn't carry its attachment along (the sealed
  copy is encrypted specifically for the original recipient's key) —
  send the file again with `sendfile` if it needs to go to someone new.
