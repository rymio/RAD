# Rymnet Install and CLI Guide

## Prerequisites

Install Rust with Cargo:

```bash
rustc --version
cargo --version
```

The project was built as a Rust workspace in:

```bash
rymnet/
```

Run all commands below from that directory unless noted:

```bash
cd rymnet
```

## Install Binaries Locally

Install the CLI:

```bash
cargo install --path crates/rym
```

Install the daemon:

```bash
cargo install --path crates/rymnetd
```

Install the interactive shell:

```bash
cargo install --path crates/rymsh
```

After installation:

```bash
rym --help
rymnetd --help
rymsh --help
```

If Cargo installs into `~/.cargo/bin`, make sure that directory is on your `PATH`.

## Run Without Installing

You can also run directly from the workspace:

```bash
cargo run -p rym -- --help
cargo run -p rymnetd -- --help
cargo run -p rymsh
```

Example:

```bash
cargo run -p rym -- init
```

## Data Directory

By default, Rymnet uses:

```text
~/.rymnet/
```

For testing multiple nodes on one machine, use `--data-dir`:

```bash
rym --data-dir /tmp/rymnet-node-a init
rym --data-dir /tmp/rymnet-node-b init
```

The same flag exists on the daemon:

```bash
rymnetd --data-dir /tmp/rymnet-node-a run
```

## First Node Setup

Initialize the node:

```bash
rym init
```

Show the node ID:

```bash
rym node id
```

Check local node status:

```bash
rym node status
```

## Domain Commands

Search for a local `.rym` claim:

```bash
rym domain search rob.rym
```

Claim a domain:

```bash
rym domain claim rob.rym
```

Show claim details:

```bash
rym domain info rob.rym
```

List local claims:

```bash
rym domain list
```

Renew/release are currently explicit prototype stubs:

```bash
rym domain renew rob.rym
rym domain release rob.rym
```

## Site Commands

Create a default site:

```bash
rym site create rob.rym
```

This creates:

```text
~/.rymnet/sites/rob.rym/source/index.html
~/.rymnet/sites/rob.rym/source/style.css
```

Upload a site folder:

```bash
rym site upload rob.rym ./website
```

Validate the current source:

```bash
rym site validate rob.rym
```

Publish the site:

```bash
rym site publish rob.rym
```

Publishing does the following:

- validates the site
- hashes all files
- writes chunks
- creates a signed manifest
- saves the latest manifest
- indexes HTML pages
- creates signed search records
- announces search records to configured peers

Show status:

```bash
rym site status rob.rym
```

List saved manifests:

```bash
rym site versions rob.rym
```

Print the local gateway URL:

```bash
rym open rob.domain
```

## Gateway

Start the daemon:

```bash
rymnetd run
```

Open the local gateway URL:

```text
http://127.0.0.1:7447/rob.rym/
```

The gateway verifies the domain claim, site manifest, and chunk hash before serving files.

## Search Commands

Local search:

```bash
rym search welcome
```

Domain-filtered search:

```bash
rym search --domain rob.rym welcome
```

Network search using signed shared search records:

```bash
rym search --network welcome
```

Index status:

```bash
rym index status
```

Rebuild index from local published/replicated sites and stored records:

```bash
rym index rebuild
```

Index one site:

```bash
rym index site rob.rym
```

Clear the index:

```bash
rym index clear
```

## Peer Commands

Add a peer:

```bash
rym peer add node-a <peer-public-key-base64> <host>:7444
```

List peers:

```bash
rym peer list
```

Ping a peer over the encrypted peer protocol:

```bash
rym peer ping node-a
```

Remove a peer:

```bash
rym peer remove node-a
```

Peer config is stored at:

```text
~/.rymnet/peers/peers.toml
```

### Bootstrapping with just an IP: `rym peer connect`

`peer add` needs the remote's public key ahead of time. If all you have is
an address, use `connect` instead:

```bash
rym peer connect 10.10.10.10
rym peer connect 10.10.10.10:7444 --name office-node   # custom name, explicit port
```

This connects and completes the encrypted handshake *without* needing to
already know the remote's key — the handshake itself reveals it, signed,
so the key you end up with is still fully verified, just discovered
instead of copy-pasted. `connect` then does one more thing: it asks that
node for its own list of known peers, and for every one it doesn't
already have, independently connects and pings it before adding it —
never just trusting the first node's word for who else is out there. In
one command you go from "one IP" to "peered with everything that node
already knows about and could verify":

```text
$ rym peer connect 10.10.10.10
Connecting to 10.10.10.10:7444...
Connected: 10.10.10.10 (rym1ba5f6...)
10.10.10.10 knows about 2 other peer(s); verifying each before adding...
  + added node-b (10.10.10.11:7444)
  - skipped node-c (10.10.10.12:7444): unreachable or key mismatch
Done: added 1 new peer(s), skipped 1.
```

This is also the practical answer to "what if the node I connected to
goes down": if `10.10.10.10` (above) later disappears, you still have
`node-b` configured from that one `connect` call, so your other commands
keep working against it without you having to do anything — see the note
on peer fan-out in "Peer Commands" above for how that already works.

A couple of things worth knowing:

- **This reveals your peer list to whoever connects and asks.** By
  default, every node answers `connect`'s peer-list request from anyone
  who can reach it — that's what makes bootstrapping easy, but it does
  mean disclosing the names/keys/addresses of your peers to strangers who
  just connect and ask, not only to peers you've vetted. Turn it off in
  `config.toml` if you don't want that:
  ```toml
  [node]
  share_peer_list = false
  ```
- **Discovered peers are always re-verified, never just trusted.** If a
  node's peer list claims an entry that turns out to be unreachable, or
  answers with a different key than claimed, `connect` skips it rather
  than adding a peer entry that can't actually be validated.

## Replication Commands

**`rym site publish <domain>` auto-replicates to every configured peer**
as its last step — you don't need to separately run `replicate` anymore.
If no peers are configured yet, publish just says so and tells you to
run `rym peer connect <ip>` first; nothing fails silently. This closes a
real gap: `publish` used to only save locally (its own last line said
so — `Status: saved locally`), which meant a freshly published site
could sit invisible to every other node until someone remembered to
replicate it by hand.

**`rymnetd` also re-announces everything it has to every peer on a
timer** — not just sites. Every 5 minutes by default
(`[replication].resync_interval_secs` in `config.toml`; not written by
default, so existing installs pick up the 5-minute default
automatically without needing to edit anything, but you can add the
line to change it), it re-pushes:

- every domain claim it has a complete local copy of (owned **or**
  merely hosted as a replica — so content spreads transitively through
  the mesh, not just outward from whoever originally published it),
  that domain's site manifest + chunks, and its search records
- every account claim, its latest vfs pointer, its mailbox key, and
  every vfs chunk this node has actually accepted for that account
- every mail message this node is relaying, for every recipient it
  holds an inbox for

This means a peer that was offline when something was originally
published/synced/sent, or that joins your mesh later via `rym peer
connect`, catches up on its own instead of staying permanently missing
that content — accounts and mail included, not just sites. It's
unconditional re-push (not a smart diff against what each peer already
has), so expect steady, modest background traffic proportional to how
much you've published/synced/sent — fine for the small sites and modest
account/mail volumes this project targets. It also only travels along
peer connections that actually exist: if two nodes never share a
common peer (direct or transitive), nothing about this makes content
reach across that gap — there's no discovery layer, only propagation
along `peers.toml`.

Fetch a site from configured peers:

```bash
rym site fetch rob.rym
```

Manually (re-)offer a local published site to peers — mostly useful now
for controlling exactly how many peers to send to, since `publish`
already does this automatically for all of them:

```bash
rym site replicate rob.rym --copies 3
```

Pin a site locally:

```bash
rym site pin rob.rym
```

Unpin:

```bash
rym site unpin rob.rym
```

Show local replica status:

```bash
rym site replicas rob.rym
```

## Two-Node Local Demo

Use two data directories:

```bash
rym --data-dir /tmp/rymnet-a init
rym --data-dir /tmp/rymnet-b init
```

On Node A:

```bash
rym --data-dir /tmp/rymnet-a domain claim rob.rym
rym --data-dir /tmp/rymnet-a site create rob.rym
rym --data-dir /tmp/rymnet-a site publish rob.rym
```

Get Node A public key:

```bash
jq -r .public_key /tmp/rymnet-a/identity/node_key.json
```

Start Node A:

```bash
rymnetd --data-dir /tmp/rymnet-a run
```

On Node B:

```bash
rym --data-dir /tmp/rymnet-b peer add node-a <node-a-public-key> 127.0.0.1:7444
rym --data-dir /tmp/rymnet-b peer ping node-a
rym --data-dir /tmp/rymnet-b site fetch rob.rym
rym --data-dir /tmp/rymnet-b site replicas rob.rym
rym --data-dir /tmp/rymnet-b search welcome
```

Network search without fetching the full site:

```bash
rym --data-dir /tmp/rymnet-b search --network welcome
```

## Interactive Shell (rymsh)

`rymsh` is an interactive shell that opens onto a personal "virtual computer"
for the local rymnet node, and gives you a small set of familiar file
commands plus a text-mode browser for `.rym` sites. It does **not** require
`rymnetd` to be running locally at all — every network-facing command
(browsing, search, accounts, mail, sync) talks directly to whichever peers
are in your `peers.toml` if there's no local daemon to use. A local
`rymnetd` is only ever a shortcut: if it happens to be running, `rymsh`
uses its gateway for browsing/search since that's a single local round-trip
instead of several network ones, but nothing breaks without it. `rymsh`
checks for one at startup just to tell you which mode you're in.

Start it:

```bash
rymsh
rymsh --data-dir /tmp/rymnet-node-a
```

### File commands (local to your virtual computer)

```text
ls [path]        pwd              mkdir <dir>
cd <path>        touch <file>     rm [-r] <path>
cp <src> <dst>   mv <src> <dst>   cat <file>
nano <file>      clear
```

`nano <file>` opens a small full-screen editor in the spirit of nano
(arrow keys to move, Ctrl+S to save, Ctrl+X to exit).

All of these commands are sandboxed to your virtual computer's storage — you
cannot `cd ..` out of it or reach the rest of your real filesystem.

### Browsing (a small lynx-style browser)

Rymnet sites are HTML/CSS only, so `rymsh` includes a minimal text browser
for them instead of shelling out to a real web browser:

```text
lynx rob.rym
lynx rob.rym/about.html
```

This fetches the page from the local gateway if one is running, or —
otherwise — resolves it directly over the peer protocol from your
configured peers (the same `site.manifest.request`/`site.chunk.request`
messages `rym site fetch` uses, with the same signature and content-hash
verification the gateway itself performs), strips it down to readable
text, and lists its links as numbered references. Type a number to follow
a link, `b` to go back, or `q` to return to `rymsh`.

Inside `lynx`, typing free text with no `.rym` in it searches the network
instead of navigating (text that looks like a domain still navigates there
directly) — see the next section.

### search.rym: a node-based search engine

`search.rym` is a permanent, reserved domain that every gateway serves
built in — it is not owned or published by anyone, cannot be squatted by a
`domain claim`, and works identically on any node. From `rymsh`, `search`
and `lynx search.rym` don't need a gateway at all: if there's no local
`rymnetd`, they query your configured peers directly for signed search
records over the peer protocol (the same mechanism `rym search --network`
uses), verify each one, and merge/deduplicate the results themselves.
Either way, results only ever reflect what's actually been indexed and
shared — there's no global crawl, just whatever sites have been published
and announced.

From `rymsh`:

```text
rymsh:/home$ search rust programming
```

or interactively:

```text
rymsh:/home$ lynx search.rym
=== search.rym ===
(search.rym/)

search .rym  Search  A node-based search engine built into every rymnet
gateway. No JavaScript, no tracking, no central index.

lynx [search.rym/]> rust programming

=== search.rym ===
(search.rym/?q=rust+programming)

2 result(s)
Rust Programming Language
http://127.0.0.1:7447/rustlang.rym/
A systems programming language focused on safety and speed.

References
  [1] rustlang.rym/

lynx [search.rym/?q=rust+programming]> 1
```

It also renders as an actual Google-style results page for a real web
browser pointed at a gateway — `http://127.0.0.1:7447/search.rym/` has a
search box (a plain static `GET` form; still no JavaScript) and clickable
result links, so you don't need `rymsh` to use it that way. That path
*does* need a `rymnetd` running somewhere reachable at that address (it's
plain HTTP, not the peer protocol) — it's only `rymsh`'s own `search`/
`lynx` commands that work with no daemon anywhere, as described above.

**A deliberate departure from the v0.1 scope of work:** the SOW rules out
"dynamic server-side apps" to keep the network limited to static HTML/CSS.
`search.rym` is one narrow, explicit, first-party exception to that — a
single hardcoded endpoint implemented in Rust in the gateway itself (not a
scripting or plugin platform, and nothing a `.rym` site's own HTML/CSS can
opt into), added because a network people actually use needs a way to find
things on it. Every other domain still goes through the same
claim-verify-manifest-verify-chunk-verify pipeline as before.

### File manager (a small Midnight Commander clone)

Once logged in, `mc` (or `files`) opens a full-screen, two-pane file
manager — the only supported way to move real files from your computer into
your encrypted rymnet account:

```text
rymsh:mcuser@/home$ mc
```

```text
 computer: /Users/you                    rymnet:mcuser: /Users/you/.rymnet/accounts/cache/mcuser
 Documents/                               home/
 sample.txt  (22 bytes)                   sample.txt  (22 bytes)
```

The **left pane is your real, unsandboxed local filesystem**; the **right
pane is your rymnet virtual computer**, sandboxed the same way the rest of
`rymsh` is. Keys:

```text
Tab          switch the active pane
Up/Down      move the selection
Enter        open a directory
Backspace    go up a directory
c / F5       copy the selected file or folder into the OTHER pane's directory
m / F7       create a new folder
n            create a new empty file
r / F2       rename
d / F8 / Del delete the selected file or folder (asks to confirm)
e / F4       edit the selected file (same nano-clone editor as `nano`)
q / F10      quit and return to the rymsh prompt
```

Copying a file from the computer pane into the rymnet pane only writes it
into your local, decrypted working copy of your virtual computer — it is
not encrypted and pushed to peers until you run `sync` (which the file
manager will remind you to do on the way out, if anything changed).

### Python (a small embedded interpreter, just for fun)

`rymsh` embeds [RustPython](https://github.com/RustPython/RustPython) so you
can write small programs against your virtual computer:

```text
rymsh:/home$ python
>>> for i in range(3):
...     print("hi", i)
...
hi 0
hi 1
hi 2
>>> write_file("greeting.txt", "hello from python")
>>> print(read_file("greeting.txt"))
hello from python
>>> exit()
```

Or run a whole script already sitting in your virtual computer:

```text
rymsh:/home$ python myscript.py
```

This is a real Python-language interpreter (loops, functions, classes,
exceptions, the usual builtins) but it is **not** a general-purpose Python —
it starts with no standard library at all, so there is no `import os`,
`import sys`, `import socket`, etc, and therefore no way to reach the real
filesystem, network, or any process on the machine. The only I/O available
is a small set of functions scoped to whatever vfs is currently active
(your logged-in rymnet account if you're logged in, otherwise the local
default virtual computer), the same one `ls`/`cat`/`nano` use:

```text
read_file(path)            write_file(path, text)      append_file(path, text)
list_dir(path='.')         exists(path)                remove_file(path)
mkdir(path)
```

Paths are sandboxed exactly like the rest of `rymsh`: `read_file("/etc/passwd")`
or `read_file("../../../../etc/passwd")` cannot escape the vfs — both
resolve to a path inside it (which won't exist, so they just raise a
`RuntimeError`), never to anything on the real disk.

### Publishing a site from your virtual computer

```text
rymsh:/home$ mkdir mysite
rymsh:/home$ cd mysite
rymsh:/home$ nano index.html
rymsh:mysite$ publish demo.rym
```

`publish <domain.rym>` chains the existing site pipeline (`rym domain
claim`, `site create`, `site upload`, `site publish`) against the current
directory in one step, instead of running each manually. It uses your
machine's own node identity (from `rym init`), the same as running those
`rym` commands directly would — this is independent of whichever `rymsh`
*account* you're logged into, since domain/site ownership and the
username+password account system are two separate identities in rymnet.
Re-running `publish` on a domain you already own is fine (it just
re-uploads and re-publishes); `rym index site <domain>` afterward makes it
show up in `search.rym`.

### Accounts: one username+password, synced across every machine you use

`rymsh` supports logging in with a username and password that work the same
way on any machine running `rymnetd`, without a central account server.
Both your node identity and a symmetric encryption key are derived from your
username and password with Argon2id — logging in with the same pair
anywhere regenerates the same keys. Every file in your virtual computer is
encrypted with that key *before* it ever leaves your machine, so peers that
store your data for you (the same way they replicate `.rym` sites) only ever
see opaque ciphertext, never file names or contents.

```text
login <username> <password>   derive your identity, pull your files from peers
logout                        push local changes to peers, end the session
sync | push                   push local changes to peers now
pull                          pull the latest version from peers now
whoami                        show the logged-in account
```

`exit`/`quit` while logged in behaves like `logout`: it pushes your changes
before leaving, so nothing is left stranded only on the local disk.

Usernames are claimed on a first-come basis and verified the same way
`.rym` domains are (a signed claim, earliest timestamp wins on conflict). If
you try to log in with a username already registered under a different
password, `rymsh` reports the conflict instead of overwriting anyone's data.

This requires at least one reachable peer configured (`rym peer add ...`
or `rym peer connect ...`, see [Peer Commands](#peer-commands)) — a local
`rymnetd` is not needed at all for accounts, since `login`/`sync`/`pull`
talk directly to your peers over the peer protocol. Your virtual computer
is never assumed to exist only on the machine you're sitting at.

### Per-account storage quota

Each node enforces a storage quota on the accounts it hosts for others, set
in `config.toml`:

```toml
[account]
enabled = true
quota_mb = 100
```

`rymsh` checks this quota locally before pushing, and every peer that
receives your encrypted files enforces it again independently before
accepting them — a `login`/`sync` that would push a virtual computer over
the limit is rejected with a clear error rather than partially uploaded.

Run `df` in `rymsh` to see how much of your quota is used:

```text
rymsh:mcuser@/home$ df
VIRTUAL COMPUTER                 SIZE       USED      AVAIL   USE%
rymnet:mcuser                100.00 MB    5.16 KB   99.99 MB   0.0%
```

`USED` is your current local files, not the last version actually pushed —
it's slightly smaller than what a `sync` would report, since each file
gains about 16 bytes of encryption overhead once it's actually encrypted
and uploaded. If you're not logged in, `df` just shows local scratch usage
with no quota, since the anonymous local vfs isn't synced to the network.

### Unlimited accounts (`god`)

A node can exempt specific usernames from its quota entirely:

```toml
[account]
enabled = true
quota_mb = 100
unlimited_usernames = ["god"]
```

`god` is exempt by default. This is a **per-node, opt-in** trust decision,
not a network-wide grant: each node operator decides locally whether to
honor it, the same way peers themselves are configured manually in
`peers.toml`. Remove `"god"` from the list (or clear it entirely) if you
don't want your node to host unlimited data for anyone. Because usernames
are still claimed on a first-come basis (see above), whoever logs in as
`god` first with a given node's trust owns that identity there — pick a
strong password for it, since (as with any account) there is no recovery
if it's lost or guessed.

`df` reports `unlimited` instead of a byte count/percentage when the
logged-in account is exempt.

### Email

Rymnet has no SMTP, no MX records, no mail server, and no central
directory — email is built entirely out of pieces that already exist:
usernames are already a public, signed directory of identities (the same
`AccountClaimRecord` that makes `login` work), and mail is delivered the
same way vfs data is synced — as an opaque encrypted blob that peers
relay for each other. The one new primitive is real public-key
encryption: your account's password also derives an X25519 keypair (in
addition to the Ed25519 identity and the vfs encryption key), and its
public half is published alongside your username claim so *anyone* can
seal a message for you without ever knowing your password. Only you can
open it.

Compose and send (`login` first):

```text
rymsh:alice@/home$ emailto rob
Cc (comma-separated, optional):
Bcc (comma-separated, optional):
Subject: Hello
Body (blank line to finish):
> Hi Rob, just saying hi.
>
Looking up rob's mailbox key...
Sent to rob: delivered to 2/2 peer(s).
```

`emailto` alone (no username) prompts for `To:` first, matching the
`emailto: rob<enter>` shorthand. What actually happens under the hood:

1. Look up `rob`'s published X25519 public key (checking this node's own
   disk first, then every configured peer) and verify it's really signed
   by whoever owns the `rob` username claim.
2. Seal the message for that key alone, using a single-use ephemeral
   keypair (like a self-destructing envelope — even the sender can't
   decrypt it again once the ephemeral key is gone) and sign the whole
   sealed envelope with your own account identity, so the recipient can
   verify who really sent it.
3. Broadcast the sealed, signed envelope to every peer you have
   configured. There's no routing table saying which peer(s) `rob`
   actually reads from, so broadcasting to your own peers is what
   maximizes the odds of reaching whichever peer he checks — the same
   tradeoff `sync` already makes for vfs data.

Check your inbox:

```text
rymsh:rob@/home$ email
Fetching inbox from 2 peer(s)...

Inbox: 1 message(s)

  #   From             Subject                              Date
  1   alice            Hello                                2026-07-10 14:32

email> 1

From:     alice
To:       rob
Subject:  Hello
Date:     2026-07-10 14:32
Verified: yes (signed by alice's registered key)

Hi Rob, just saying hi.

email message [1]> r
Replying to alice. Subject: Re: Hello
Body (blank line to send):
> Hey! Good to hear from you.
>
Sent.
```

At the message prompt: `r` replies (pre-fills `Re:` and threads the
message), `f` forwards to a new recipient (prompts for an optional note,
then quotes the original underneath), and `q` returns to the inbox list.
`Verified: yes` means the envelope's signature checks out *and* the
signing key matches that username's current registered claim — a
message claiming to be from someone but signed by a different key shows
`Verified: no` instead of being silently trusted.

A few things worth knowing:

- **Peers relay mail for anyone, opaquely.** The same way a node hosts
  encrypted vfs chunks for accounts it doesn't own, any node will hold a
  little mail on behalf of any username without verifying it owns that
  account — the content is sealed, so there's nothing for the relay to
  read anyway. This is bounded by its own quota, `[mail].quota_mb` in
  `config.toml` (default 20 MB per recipient), independent of the vfs
  quota so a full mailbox and a full vfs don't compete with each other.
  `[account].unlimited_usernames` (see "Unlimited accounts" above)
  exempts the same names from this quota too.
- **Bcc is actually blind.** It travels inside the sealed payload like
  everything else, so a relaying peer never sees it, and `email`'s
  message view never prints it to the primary recipient — only the
  sender's own sent copy would show it (sent mail isn't kept anywhere
  yet; see below).
- **What's *not* here yet:** a sent-mail folder, deleting/expiring old
  messages from peers, and network-wide mail search. Read mail currently
  stays wherever it was relayed to — there's no client-side "mark as
  read and remove" round-trip back to peers.

### Graphical Desktop (`window` / `gui`)

If you're at a real desktop (not a headless server), `window` (alias
`gui`) launches `rymwin` — a separate graphical app, a small retro
desktop shell with its own login (see [`rymwin`](rymnet/crates/rymwin/)).
It needs to be installed once (`cargo install --path crates/rymwin --force`,
run from inside the `rymnet/` workspace directory) and on `PATH`.

```text
rymsh:/home$ window
opening Rymnet Desktop...
```

`rymwin` logs in independently of `rymsh` — it derives its own keys from
your password rather than inheriting the running `rymsh` session, so
nothing is shared between the two beyond the same peer network. On
Linux, `rymsh` checks for `$DISPLAY`/`$WAYLAND_DISPLAY` before trying and
tells you plainly if there's no display to open a window on (e.g. an SSH
session with no X forwarding); on macOS/Windows it just attempts the
launch. The spawned process is fully detached (its own process group,
no inherited stdio) — a `rymwin` window needs to keep running its own
event loop after `rymsh` exits, and staying attached to `rymsh`'s
process group/session reliably crashed the freshly opened window the
moment `rymsh` exited, on macOS at least.

Once inside `rymwin`, the Start button (bottom-left) opens **File
Manager** (folder tree, CRUD), **Rymcode** (a small editor for writing
and running RymScript programs — see below), **Browser** — a small
early-Internet-Explorer-style web browser for `.rym` sites: Back/
Forward/Refresh/Home toolbar, an address bar, and a status bar. Type a
domain (`example.rym`) to fetch it, or plain text with no ".rym" in it
to search the network instead — same convention as `rymsh`'s `lynx` and
`rymweb`. Since rymnet content is HTML/CSS only (no JavaScript, no
forms — see "Important Safety Rules" below), there's nothing to execute,
just a title, body text, and clickable links to follow — **Terminal**, a
black-background command line offering most of `rymsh`'s command set
(`ls`/`cd`/`pwd`/`mkdir`/`touch`/`rm`/`cp`/`mv`/`cat`/`edit`, `lynx`/
`search`, `run`, `publish`, `sendfile`) against the same already-logged-in
session, no separate login needed inside it. It's not a real terminal
emulator (no PTY, no ANSI/VT100 rendering) — it reuses `rymwin`'s own
command logic directly, the same pattern `rymweb`'s browser-based
terminal already uses. `run <name>` supports genuinely interactive
`input()` for `.run` scripts (the starter `hello.run` example needs it),
not just captured output like Rymcode's preview. Not yet in the
terminal: email compose/read (use the Mail app instead — see below) and
`mc` (use File Manager instead) — typed plainly in its own `help` output
rather than left to be discovered by trial and error — plus **Mail**,
**Chat**, **PDF Reader**, **Paint**, **Writer**, and **Board**, covered below.

**Mail** is an Outlook-alike client: folder list (Inbox / Sent) on the
left, message list, reading pane below it, and a proper compose window
(To/Cc/Bcc/Subject/Body all editable at once, not a sequential prompt).
Reply and Forward pre-fill from the selected message the same way
`rymsh`'s `email` command does. Two things are honestly not real,
stated plainly in the app rather than hidden: **Sent** is only this
session's own outgoing messages, kept in memory — there's no real
"sent items" concept anywhere in Rymnet's mail design — and unread/read
marking is local-only styling, not synced. `Verified: yes` on a message
means the same thing it means everywhere else in this project: the
signature checks out *and* the signing key matches the sender's current
registered claim, not just that the signature is internally consistent.
Selecting a message with a file attached (see the File Transfer Guide)
shows a **Save as** field and button to fetch, unseal, and save it into
your vfs.

**Chat** is `rymirc`'s desktop client: a channel list on the left, a
scrollback + compose box for the active channel, and a **Direct
Message...** button for one-to-one sealed mail under a chat-styled
input. See the IRC / Chat Guide for the full reference.

### PDF Reader (optional native dependency)

**PDF Reader** renders real PDF pages — actual layout, images, and text,
not just extracted text — using `pdfium-render`, a Rust wrapper around
Pdfium, the same PDF engine Chrome itself uses. This is the **one and
only native, non-Rust runtime dependency anywhere in this project**;
every other crate in this workspace is pure Rust. That trade was made
deliberately, after checking: the one pure-Rust PDF page rasterizer that
exists on crates.io has a few hundred downloads and is still in beta,
while `pdfium-render` has been downloaded well over a million times and
wraps a production PDF engine — there wasn't a mature pure-Rust
alternative worth building on for something meant to render arbitrary
real-world PDFs correctly.

`rymwin` builds and runs completely normally without this — every other
app is unaffected, and PDF Reader's own window opens fine, too. It's
only *opening a PDF* that needs Pdfium actually present, and if it
isn't, you'll see a clear message in the app's window rather than a
crash: *"Pdfium library not found — see INSTALL_AND_CLI_GUIDE.md's PDF
Reader section"* — which is this section.

**One-time setup:**

1. Download a prebuilt Pdfium library for your platform from
   <https://github.com/bblanchon/pdfium-binaries/releases> (grab the
   release matching your OS/architecture, e.g. `pdfium-mac-arm64` on
   Apple Silicon).
2. From the archive, take the shared library file and place it in the
   same folder you launch `rymwin` from (or right next to the installed
   binary), named exactly:
   - macOS: `libpdfium.dylib`
   - Linux: `libpdfium.so`
   - Windows: `pdfium.dll`

That's it — no rebuild needed. `rymwin` looks for the library in its
current working directory first, falling back to a system-installed
copy if one exists.

Open PDF Reader from the Start menu, browse your vfs in the file tree
on the left (the same tree-browsing pattern Rymcode uses), and click a
`.pdf` file to open it. Prev/Next page and Zoom −/+ are in the toolbar;
a password-protected PDF shows a password field instead of a blank
page. Rendering happens on its own background thread so the rest of the
desktop stays responsive while a page is being rasterized.

### Paint

**Paint** is a simple pixel editor for PNG images — needs no setup,
unlike PDF Reader; it's pure Rust (the `image` crate) like everything
else in this project. Open it from the Start menu, browse your vfs in
the same file-tree pattern PDF Reader and Rymcode use, and click a
`.png` file to open it, or click **New** for a blank canvas at a size
you choose. Tools: Pencil, Eraser (paints true transparency, not white),
and Fill (flood fill from a click, exact color match) — pick a color
with the built-in color swatch button, adjust brush size with the
slider, and **Undo** steps back through up to 20 strokes. **Save**
writes back to the file you opened; **Save As** picks a new vfs path.

### Writer

**Writer** is a markdown editor, extension `.wrr` — needs no setup, pure
Rust like Paint. It's markdown source on the left, a live rendered
preview on the right (toggle to Source-only or Preview-only from the
toolbar), with toolbar buttons for Bold/Italic/Code, H1–H3, bullet/
numbered lists, quotes, links, and inserting images from your vfs (the
same file-tree picker every other app uses). Undo/redo is `TextEdit`'s
own built-in per-widget history (ctrl+Z/ctrl+Y) — nothing custom.

The point of Writer is **Export as HTML**: pick a target path (e.g.
`/sites/mysite/index.html`) and it writes out real HTML, a small default
`style.css` alongside it, and copies of every image the document
references — a complete, self-contained site folder. From there it's the
same publishing flow as any other site: `cd` into that folder in
Terminal (or `rymsh`) and run `publish <domain.rym>` — see "Publishing a
site from your virtual computer" above. Writer doesn't publish anything
itself; it only produces the HTML+CSS+images `publish` already knows how
to take from there.

### Board

**Board** is a simple, no-voting Reddit-style forum — needs no setup,
pure Rust like Paint/Writer. Named boards (not claimed or owned by
anyone, same as chat channels) hold text posts (a title, a body, and an
optional link) with flat, one-level comment replies — no nested
comment-on-comment threading. Posts and comments are signed but not
encrypted, the same "verified origin, not confidentiality" model chat
already uses, and relayed/polled the same way too — there's no voting or
karma at all.

Join or create a board by typing its name in the left-hand list and
clicking **Join** (a board is created the moment its first post lands,
same as a chat channel). Click a post to open its thread; **New Post**
composes a title/body/optional-link post to the active board; typing in
the box at the bottom of an open thread and pressing Enter (or
**Comment**) posts a reply. **Back** returns to the post list.

Board is built to hold up at thousands of posts without becoming
sluggish: each board keeps a small, append-only index of post summaries
(title/author/time) that listing reads instead of scanning every full
post file, and the desktop app's post list only ever renders the rows
actually scrolled into view (`egui::ScrollArea::show_rows`), not the
whole list every frame. The trade-off worth knowing: since that index is
unsigned (a local convenience cache, not something cryptographically
proven), the list itself trusts whichever peer answered — the same
posture every other fetch in this project already has — and is
independently verified for real the moment you actually open a post.

The same feature is available from `rymsh` as `board` (alias `forum`):
`/board <name>` to switch/join, Up/Down + Enter to select and open a
post, `/new <title> | <body>` to post, plain text inside a thread to
comment, `/back` to return to the list.

### RymScript programs (`run`, Rymcode, `/Programs`)

Every account gets a `/Programs` folder the first time it logs in
(from either `rymsh` or `rymwin`), seeded with four examples: basic
`hello.run`/`counter.win`, plus `checksum_tool.run` and `notes.win`,
which exercise RymScript's curated standard library (hashing, random,
csv, and JSON — see below). RymScript is Python syntax on a sandboxed
interpreter, with two program types:

- **`*.run`** — terminal programs. Run them from `rymsh`:
  ```text
  rymsh:/home$ run hello
  Hello from RymScript!
  What's your name? Ada
  Nice to meet you, Ada.
  (wrote notes.txt in your home folder)
  ```
  Same sandboxed file API as `python <file>` (`read_file`/`write_file`/
  `append_file`/`list_dir`/`exists`/`remove_file`/`mkdir`), plus normal
  `print()`/`input()` since a real terminal is right there. Naming a
  `.win` file here gets a clear redirect message instead of a confusing
  "no such file".
- **`*.win`** — desktop programs. Run only inside `rymwin`, from
  Rymcode's Run button or File Manager. A `.win` script declares its UI
  each frame as a list of widget dicts (label, button, text input,
  checkbox, ...) and reacts to what happened last frame via an `events`
  dict — see [`RYMSCRIPT_SDK.md`](RYMSCRIPT_SDK.md) for the full
  reference and the `counter.win` example worked through in detail.

Beyond the file API, both program types can `import` a curated slice of
the standard library — `math`, `cmath`, `unicodedata`, `zlib`,
`binascii`, `csv`, `struct`, `random`, and per-algorithm hashing
(`sha256`, `md5`, `sha1`, `sha3`, `sha512`, `blake2`) — plus
`json_loads`/`json_dumps` as plain functions. No real filesystem,
network, or process access exists from either program type; see
`RYMSCRIPT_SDK.md`'s "Standard library" section for the full list, the
handful of modules that use a different calling convention than real
Python (`random`, hashing), and why some obvious-looking modules
(`json`, `bisect`, `statistics`, `re`) aren't available at all.
`checksum_tool.run` and `notes.win` are worked examples of a `.run` and
a `.win` app built on this, ready to open in `rymsh`/Rymcode.

Rymcode (open it from the Start menu, or from `rymwin`'s File Manager)
is a small single-file editor: a folder tree to browse `/Programs` (or
anywhere else in your vfs), Save, and a Run button that behaves
differently per extension — `.win` files launch as a real desktop app
window immediately; `.run` files execute in a non-interactive preview
pane inside Rymcode itself (captures `print()` output; `input()` isn't
available there — use `rymsh`'s `run` command for anything interactive).

### Design view: a visual, drag-and-drop-free form designer for `.win` apps

Any `.win` file gets a **Code**/**Design** toggle in Rymcode's toolbar.
Design view shows the app's widgets as a clickable tree (indented for
`row`/`column`/`grid` nesting) with a Component Palette (add a Heading,
Button, Grid, ...) and a Property Inspector (id, text, color, enabled,
grid columns) alongside it — build a form without hand-typing widget
dicts. **Double-clicking a widget** switches to Code view with an empty
handler stub already created and the cursor placed inside it (`def
on_<id>_click(events): pass`) — the "drop a widget, write the code that
runs for it" workflow, adapted to RymScript's actual execution model
(see below for what that adaptation means in practice).

Start a new one from **New File → Blank Form (.win)**, which opens
directly in Design view with nothing to hand-write first. An *existing*
hand-written `.win` script (`calculator.win`, `minesweeper.win`, and
similar) shows up in Design view too, but **read-only** — Rymcode only
ever edits the specific auto-generated block a Design-view file keeps
(clearly marked with `# --- rymcode:layout:begin/end ---` comments), and
a hand-written script doesn't have one; editing it visually would mean
guessing how to safely rewrite arbitrary logic, which Rymcode doesn't
attempt. Design-view files can still contain plenty of hand-written
code alongside the generated block — a widget's live value (a counter's
count, a clock's `now()`) comes from a plain function you write yourself
named after its id (`def text_myid(): return f"Count: {count}"`,
`def enabled_myid(): return not busy`), called automatically each frame
if it exists; this is real Python you write, not a template-expression
language.

One honest limitation worth naming directly: RymScript's `.win` model
redeclares its *entire* UI every frame (the same immediate-mode
execution real `egui` itself uses), not Delphi/VB's retained-mode model
where a form's components are live objects with properties you mutate
directly from event-handler code. Design view delivers that same
*workflow* — drag a widget on, double-click it, write its behavior — on
top of RymScript's actual execution model, not a literal port of
Delphi's object model.

## Web Terminal (`rymweb` / rymtech.net)

`rymweb` is a second, separate binary (crate `rymnet-web`) that gives
anyone with a browser the same functionality as `rymsh`, without
installing anything. It is not part of `rymnetd`'s loopback-only gateway
— it's a standalone service meant to be run publicly (e.g. as the backend
for a `rymtech.net`-style landing page).

**Trust tradeoff, stated plainly:** logging in through `rymweb` sends
your password to that server, the same way logging into webmail does. It
derives your account/vfs/mailbox keys in server memory for the life of
your browser session, never writes the password or keys to disk or logs
them, and drops them on `logout` or after an idle timeout — but whoever
operates that particular `rymweb` instance is, for that session, in a
position to see what you type. If you're not comfortable with that, use
native `rymsh` instead, where the password never leaves your machine.

### Running it locally (testing)

```bash
cargo install --path crates/rymnet-web --force   # installs the `rymweb` binary

# rymweb needs its own node identity + peers.toml, same as any node:
rym init                       # if you haven't already, in the data dir rymweb will use
rym peer connect <known-ip>    # give it at least one peer to talk to

rymweb --data-dir ~/.rymnet --listen 127.0.0.1:8080
```

Then open `http://127.0.0.1:8080/` in a browser. This is fine for local
testing but **do not** bind `--listen` to `0.0.0.0` or a public address
directly — see the production deployment guide below. The login flow
sends a plaintext password to whatever `--listen` is bound to, and that
must never happen over plain HTTP or be reachable without TLS in front of
it.

### Deploying it publicly (reverse proxy + TLS)

`rymweb` has no TLS of its own by design — it's meant to sit behind a
reverse proxy that terminates HTTPS. Ready-to-use config templates are in
[`rymnet/deploy/`](rymnet/deploy/): `rymweb.service` (systemd),
`rymnetd.service` (systemd, if this host also runs a peer node),
`Caddyfile` (simplest — automatic Let's Encrypt), and
`nginx-rymweb.conf` (nginx + certbot alternative).

**Ports:** only one port needs to be proxied — `rymweb`'s own local HTTP
port (`127.0.0.1:8080` in the templates). The proxy exposes `443`
(HTTPS) and `80` (ACME challenge + redirect to HTTPS) publicly and
forwards to that local port. This is unrelated to `rymnetd`'s peer port
(`7444`, raw Noise-encrypted TCP, not HTTP) — if this host is also a
Rymnet peer node, `7444` stays open on the firewall directly, not routed
through the reverse proxy. `rymnetd`'s gateway (`7447`) and admin
(`7448`) stay loopback-only as always and are never exposed either way.

Steps, using Caddy (simplest option — it gets and renews the certificate
itself):

1. **DNS.** Point an A (and/or AAAA) record for `rymtech.net` (or
   whatever domain/subdomain you're using) at the server's public IP.
   Wait for it to propagate (`dig rymtech.net` should show the new IP).

2. **Firewall.** Open `80` and `443` publicly. If this host also runs
   `rymnetd` as a peer, open `7444` too. Do **not** open `8080` (or
   whatever port `rymweb --listen` binds) — it should only be reachable
   from `127.0.0.1`.

3. **Install and configure `rymweb` as a service.**
   ```bash
   sudo useradd -r -m -d /home/rymnet rymnet   # dedicated service user
   cargo install --path crates/rymnet-web --force
   sudo cp ~/.cargo/bin/rymweb /usr/local/bin/rymweb
   sudo -u rymnet rym init --data-dir /home/rymnet/.rymnet
   sudo -u rymnet rym peer connect <known-ip> --data-dir /home/rymnet/.rymnet
   sudo cp rymnet/deploy/rymweb.service /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now rymweb
   sudo systemctl status rymweb   # confirm it's listening on 127.0.0.1:8080
   ```

4. **Install Caddy and point it at the domain.**
   ```bash
   # install per https://caddyserver.com/docs/install
   sudo cp rymnet/deploy/Caddyfile /etc/caddy/Caddyfile
   # edit the domain in it if it's not rymtech.net
   sudo systemctl enable --now caddy
   sudo systemctl reload caddy
   ```
   Caddy handles the ACME HTTP-01 challenge on `:80` and issues/renews
   the certificate automatically — no separate certbot step. (If you'd
   rather use nginx + certbot instead, use `nginx-rymweb.conf` and run
   `sudo certbot --nginx -d rymtech.net` after nginx is up and serving
   the plain-HTTP version of that config once.)

5. **Verify end to end.**
   ```bash
   curl -I https://rymtech.net/         # expect HTTP/2 200
   ```
   Then open `https://rymtech.net/` in a browser, confirm the padlock is
   valid, and run through `help` → `login` → `ls` to confirm the whole
   chain (browser → Caddy → rymweb → peer network) works.

6. **If this host is also a peer node**, install `rymnetd.service` the
   same way (`sudo cp rymnet/deploy/rymnetd.service
   /etc/systemd/system/ && sudo systemctl enable --now rymnetd`) — it's
   independent of `rymweb` and can point at the same `--data-dir` or a
   different one.

### What works

The full `rymsh` command set except two things (see below):

```
help                                overview
login <user> <pass>   logout        (password never persisted)
sync | push            whoami        df
ls [path]  cd <path>  pwd  mkdir <dir>  touch <file>
rm [-r] <path>  cp <src> <dst>  mv <src> <dst>  cat <file>
edit <file>                         multi-line editor, blank line to save
lynx <domain.rym>[/path]            search <query>
emailto <user>                      guided compose: Cc, Bcc, Subject, body
email  email read <n>  email reply <n>  email forward <n>
```

Sessions are tracked with an `HttpOnly` cookie the server sets on first
contact; a not-logged-in visitor gets an ephemeral sandboxed "guest"
virtual computer that's discarded when the session expires (default idle
timeout: 2 hours).

### What's not there yet

- `mc` (the file manager) — needs real local filesystem access a browser
  sandbox can't grant
- the embedded Python REPL — RustPython's VM isn't safely reusable across
  independent requests landing on different server threads
- `pull` as a standalone step — `logout` then `login` again re-pulls the
  latest vfs, since that's already part of the login flow

Both `mc` and the Python REPL remain available in native `rymsh`.

## Important Safety Rules

Rymnet v0.1 rejects unsafe web content. Do not include:

- JavaScript
- WASM
- server-side scripts
- forms
- iframes
- object/embed tags
- inline event handlers
- remote CSS imports
- executable files

Site publish

Yes, two ways:

1. publish <domain> (the easy way) — does everything in one step, using your current vfs directory as the site source:


rymsh:/home$ mkdir mysite
rymsh:/home$ cd mysite
rymsh:mysite$ nano index.html
rymsh:mysite$ publish rob.rym
This chains domain claim → site create → site upload → site publish for you.

2. Direct passthrough — domain, site, peer, search, index, node, version, init, open are all forwarded straight to the rym CLI, so you can type any of the commands from that doc section verbatim at the rymsh prompt:


rymsh:/home$ domain claim rob.rym
rymsh:/home$ site create rob.rym
rymsh:/home$ site publish rob.rym
One thing to watch with #2: those passthrough commands don't understand vfs paths — site upload rob.rym <folder> needs a real filesystem path (relative to wherever you launched rymsh from), not a path inside the sandboxed virtual computer. That mismatch is exactly why I built publish: it resolves your vfs directory to its real on-disk path before calling rym site upload, so publish is the one that actually understands "the folder I'm in inside rymsh."


Start path:

rym init                                          # local identity only, starts nothing
rym peer add remote <their-public-key-b64> <their-ip>:7444
rym peer ping remote  

