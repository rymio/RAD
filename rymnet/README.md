# Rymnet

https://rymtech.net



**An encrypted, decentralised layer of the internet — a network of self-replicating peer nodes and client software for publishing sites, communicating, and running small apps, with no central server, no company in the loop, and no platform that can revoke your account.**

Rymnet is a Rust workspace implementing a full peer-to-peer protocol and the client software to use it: a CLI, a peer/gateway daemon, an interactive shell, a graphical desktop, a browser-based terminal, and a small sandboxed app runtime. Everything is verified by cryptography — identity, ownership, content integrity, search results — rather than trusted on anyone's word.

## Why this exists

Most of today's internet runs through a handful of platforms that sit between every reader and every writer: they decide what's visible, log what you do, and can revoke your account or your audience unilaterally. That's a business-model choice, not a technical requirement. Rymnet is built around a different premise — publish and communicate over a network where the only thing anyone has to trust is math, not a company's policy page.

Concretely, that means:

- **No central server.** Anyone can run the same open-source binaries and become a full, equal peer — not a customer inside someone else's system.
- **Everything verified, nothing assumed.** Domain ownership, site content, search results, account identity, and mail are all signed and/or content-hashed. A node re-verifies all of it on every request; it never just trusts its own disk or a peer's claim.
- **Client-side encryption.** Personal account data (files, mail) is encrypted with a key derived from your own username and password *before* it ever leaves your machine. Peers that store or relay your data for you only ever see ciphertext.
- **Static content only.** Published sites are HTML/CSS, nothing else — no JavaScript, which by construction means no ad networks, no tracking scripts, no engagement-optimized feeds.

Rymnet is part of RAD: it lives in RAD's `rymnet/` folder as its own Cargo workspace, and RAD's **RYMNET** view drives it (Start RYMWIN, Initiate RYMWIN, and a chat agent). User guides are in [`docs/`](docs/) — start with [`docs/RYMWIN_USER_GUIDE.md`](docs/RYMWIN_USER_GUIDE.md). See the live site at [rymtech.net](https://rymtech.net/) for the project manifesto and user-facing docs.

## What's in this repo

A Cargo workspace (this folder), split into small, single-purpose crates:

| Crate | What it is |
|---|---|
| `rymnet-core` | Node identity (Ed25519), config, shared types. |
| `rymnet-domain` | `.rym` domain claims — signed, first-come-first-served ownership. |
| `rymnet-storage` | Content-addressed chunk store, BLAKE3 hashing. |
| `rymnet-site` | Site manifests: validate, hash, chunk, sign, publish. |
| `rymnet-account` | Username+password accounts: Argon2id key derivation, encrypted virtual-computer storage, quotas. |
| `rymnet-mail` | Sealed-box (X25519) end-to-end encrypted mail with no mail server. |
| `rymnet-search` | Local search index (Tantivy) plus signed, shared search records. |
| `rymnet-network` | The peer protocol: Noise-encrypted sessions, peer discovery, replication. |
| `rymnet-gateway` | Serves published `.rym` sites over HTTP, re-verifying claim/manifest/hashes on every request. |
| `rymnetd` | The peer + gateway daemon binary. |
| `rym` | The raw CLI: node, domain, site, gateway, search, peer, and replication commands. |
| `rymsh` | An interactive shell: a sandboxed virtual computer, a lynx-style browser, accounts, mail, an embedded Python interpreter, and a small file manager. |
| `rymwin` | A graphical desktop (File Manager, Rymcode editor, Browser, Terminal, Mail, Chat, PDF Reader, Paint, Writer, Board) with its own login. |
| `rymnet-web` (`rymweb`) | A browser-based version of `rymsh` — the same functionality with nothing to install, for a public-facing landing page. |

Two more directories worth knowing about: [`starter_programs/`](starter_programs/) holds the example **RymScript** programs (`hello.run`, `counter.win`, `checksum_tool.run`, `notes.win`) seeded into every fresh account, and [`deploy/`](deploy/) has ready-to-use `systemd`/Caddy/nginx configs for running `rymweb`/`rymnetd` publicly behind TLS.

## Trust model, in one rule

```
Never trust content from a peer unless signatures and hashes verify.
```

| Layer | Mechanism |
|---|---|
| Node identity | Ed25519 keypair; node ID is a BLAKE3 hash of the public key. |
| Domain/account ownership | Signed claim, earliest timestamp wins on conflict — no registrar. |
| Content integrity | BLAKE3 chunk hashes, re-verified on every serve, not cached blindly. |
| Search trust | Signed search records, independently verified before being merged in. |
| Transport | Noise-encrypted peer-to-peer sessions. |
| Account privacy | Client-side XChaCha20-Poly1305 encryption; hosting peers see ciphertext only. |
| Mail confidentiality | Sealed-box public-key encryption (ephemeral X25519 + recipient's key). |
| Mail authenticity | Every envelope signed by the sender's account identity, cross-checked against their current username claim. |

## Quick start

```bash
cd rymnet   # inside RAD
cargo install --path crates/rym
cargo install --path crates/rymnetd
cargo install --path crates/rymsh

rym init                     # generate this node's identity
rym peer connect <known-ip>  # bootstrap into the network from one address

rym domain claim yourname.rym
rym site create yourname.rym
rym site publish yourname.rym

rymnetd run                  # then browse http://127.0.0.1:7447/yourname.rym/
```

Or skip straight to the interactive shell, which wraps the whole publish pipeline behind one command and adds accounts, mail, search, and a small app runtime on top:

```bash
rymsh
```

Full CLI reference, `rymsh` command reference, and the RymScript app SDK live in `docks/` locally (kept out of this repo's published tree; ask the maintainer for the current copies).

## Status

This is a working v0.1 prototype, not a finished product. Honest current limits: peer discovery is manual (via `peers.toml`), there's no gossip layer that finds nodes beyond your configured peers' configured peers, search ranking is a straightforward BM25 index rather than anything more elaborate, and accounts have no password reset (the username+password pair *is* the key material, by design, with no central server that could reset it for you). None of these are hidden — they're the kind of gaps that get closed by more people running nodes, not by a company shipping a feature update.

What's implemented and working today: node identity and domain claims; site publishing with full manifest/chunk verification; a local gateway; local and network search; peer networking with Noise encryption and one-command bootstrapping (`rym peer connect`); automatic + periodic replication; encrypted username+password accounts synced across machines; sealed, signed email with no mail server; a ten-app graphical desktop (`rymwin`); and **RymScript**, a curated, sandboxed Python dialect (with a real, hand-picked standard library — math, hashing, `random`, `csv`, `struct`, JSON — but no filesystem/network/process access) for writing small terminal and desktop apps.

## License

MIT — see [`LICENSE`](LICENSE).
