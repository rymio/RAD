# RYMWIN User Guide

RYMWIN (the Rymnet Desktop) is a small desktop for Rymnet, the encrypted, decentralised network built into RAD. RYMWIN has its own apps: File Manager, Rymcode, Browser, Terminal, Mail, Chat, PDF Reader, Paint, Writer and Board. It runs **inside RAD**: open **RYMWIN** in the left sidebar. You log in to it with a Rymnet account.

Rymnet's home on the web is **rymtech.net**, which is also the start page of RAD's Web Browser and Chromium. It has the Manifesto, a plain-language Overview, Getting Started, guides for the `rym` CLI, rymsh, Chat, File Transfer and RymScript, an FAQ, and "Host your own Node". **console.rymtech.net** runs a full rymsh session in any web browser with nothing to install. Logging in there sends your password to that server, like webmail does.

This guide covers how to start RYMWIN from RAD, join the network, log in, and use each app. The last sections are a command reference and troubleshooting.

---

## 1. Rymnet in one minute

- **No central server.** Every computer that runs Rymnet is a *node*, and nodes are equal *peers*. Nobody can switch off your account or your site.
- **Everything is verified.** Accounts, `.rym` domains, websites, mail and search results are signed or hashed, and every node re-checks them. Peers are never trusted on their word.
- **Your data is encrypted before it leaves your computer.** Your files and mail are encrypted with keys made from your username and password. Peers that store them for you only see unreadable data.
- **Websites are plain HTML and CSS.** There is no JavaScript, so there are no trackers or ads.

Words you'll see:

| Word | Meaning |
|---|---|
| **Node** | This computer's Rymnet identity (a key pair). Created once; its ID looks like `rym1330…`. |
| **Daemon** (`rymnetd`) | The background program that lets other nodes reach you, and serves `.rym` sites to your own browser. |
| **Peer** | Another node yours knows about. You need at least one peer for accounts, mail, chat and browsing to work. |
| **Account** | A Rymnet username + password. Works on every node. Separate from the node identity. |
| **Virtual computer** | Your account's private, encrypted file space (what File Manager shows). It follows you to every machine you log in on. |
| **`.rym` domain** | A website name on Rymnet, e.g. `alice.rym`. First come, first served; owned by the node that claimed it. |

Ports: **7444** is the peer port that other nodes connect to. **7447** is the local website gateway, and only this computer can use it.

---

## 2. Getting started from RAD

RAD has two Rymnet entries in the left sidebar, under Video:

- **RYMNET**: the control page. Join the network, see your node and peers, and talk to the assistant.
- **RYMWIN**: the Rymnet Desktop itself, running inside RAD.

The RYMNET page has three parts.

### 2.1 Initiate RYMWIN: join the network

1. Type the address of another Rymnet node into the box, as `host` or `host:port`. The port is **7444** by default. Examples: `139.99.155.167` or `node.example.com:7444`.
2. Press **Initiate RYMWIN**.

RAD then:

1. **Creates this computer's node identity** if it doesn't have one yet (`rym init`). This only happens once.
2. **Starts the daemon** (`rymnetd`) if it isn't running.
3. **Connects to the node you typed** (`rym peer connect`). That node also shares the peers *it* knows, and RAD checks each of them and adds them too. One address is usually enough to join the whole network.

Each step is shown below the button. You can press Initiate with an empty box just to create the identity and start the daemon.

Once you've joined, RAD also opens **rymsh**, the Rymnet shell, as a tab in RAD's **Terminal**, next to your other terminals. Use **Open rymsh** (or the rymsh button on either Rymnet page) to go to it. In rymsh you can `login`, `sync`, `email`, `emailto`, `mc` (file manager), `lynx` (text browser), `python`, `run`, `publish`, `df` and more; type `help` for the list.

The **status** below shows your Node ID (with a copy button), your peer port, and whether the daemon is running. "Running (started by RAD)" means RAD will stop it when you quit RAD. "Running (started outside RAD)" means someone ran `rymnetd run` themselves, and RAD leaves it alone.

### 2.2 Start RYMWIN: open the desktop

Click the **Start RYMWIN** icon on the RYMNET page, or **RYMWIN** in the sidebar. The Rymnet Desktop appears inside RAD's window and fills the space beside the sidebar. It keeps running while you use other parts of RAD, so you can come back to it, and it stops when you quit RAD.

- **Keyboard and mouse** work as in any app. Copy (⌘C), cut (⌘X) and paste (⌘V) use your Mac's clipboard, so you can copy between RYMWIN and RAD or other apps. ⌘A, ⌘Z and ⌘Y go to RYMWIN's editors. RAD's own shortcuts (⌘K, ⌘N, ⌘\…) still work.
- **Toolbar** (top): **Add files**, **Add folder**, **From vault**, a rymsh button, a RYMNET button, and **Restart**.
- If RYMWIN stops, press **Start RYMWIN** in the middle of the view.

### 2.3 Getting files into RYMWIN

Every way of copying files in puts them in your account's **/Downloads** folder, and a File Manager window opens there. You must be logged in to RYMWIN first; otherwise it asks you to log in and drop again.

- **Drag and drop** files or folders from Finder or your desktop onto the RYMWIN view. A "Drop to copy into /Downloads" overlay shows while you drag.
- **From RAD's Files view:** select files or folders and press **Send to RYMWIN**.
- **From your RAD vault:** press **From vault** in RYMWIN's toolbar, pick documents from any project, then press **Add**.
- **Add files / Add folder** in RYMWIN's toolbar opens a file picker.

If a file with the same name is already there, the new one is saved as `name (2).ext`. Copied files are part of your encrypted virtual computer and go to your peers when you log out (or `sync` in rymsh).

### 2.4 Peers

The **Peers** list shows every node you're connected to: its name, address and public key. The bin button removes a peer from your list. That only changes *your* list, not theirs.

### 2.5 The RYMNET assistant

The RYMNET page has an AI chat (see section 10). Ask it questions such as "how do I publish a website?". It can also do things for you, for example "connect me to 139.99.155.167" or "claim alice.rym".

---

## 3. Logging in to RYMWIN

RYMWIN starts on a login box: **Username**, **Password**, **Log In**.

- **First time with a new username?** Type the username and password you want and press **Log In**. That *creates* the account and registers the username on the network. There is no separate sign-up.
- **Already have an account?** Log in with the same username and password on any computer. Your files, mail and settings come with you, because the keys are rebuilt from your username and password.
- **Usernames are first come, first served.** If someone already registered that name with a different password, you'll see "username '…' is already registered under a different password". Pick another name.
- **There is no password recovery.** Nobody holds a copy of your password or keys, so choose a strong password and keep it safe.
- **You need at least one peer** to log in, because logging in fetches your data from peers and publishes your account. If you see "no peers configured", use **Initiate RYMWIN** in RAD first.

To leave, use **Log Out** in RYMWIN's title bar (the navy bar at the top of the desktop). Logging out *pushes your changes to your peers* first, so nothing is left only on this computer. Wait for "Logging out…" to finish before closing the window.

Your account is separate from this computer's **node identity**. The node owns `.rym` domains and websites you publish from this machine. The account owns your files and mail.

---

## 4. The desktop

- **Start button** (bottom left) opens the menu of apps. Before you log in, apps show "(log in first)".
- **Taskbar** (bottom) shows open windows and the clock.
- App windows can be moved, resized and closed inside the desktop. The desktop grows and shrinks with RAD's window.
- Links that open web pages open in RAD's Web Browser.

---

## 5. The apps

### File Manager
Browse your virtual computer: a folder tree, plus **New Folder**, **New File**, rename and delete. Every account starts with a `/Programs` folder of example programs. Opening a `.win` program from here runs it as a desktop app.

### Rymcode
A small code editor for **RymScript** programs (Python syntax, sandboxed):
- Browse files in the tree on the left, edit, and **Save**.
- **Run**: `.win` files open as a real desktop window. `.run` files run in a preview pane, which shows `print()` output but can't take `input()`; use Terminal's `run` for interactive programs.
- **Code / Design** toggle for `.win` files. Design view lets you build a form from a component palette (Heading, Button, Grid…) and a property inspector. **Double-click a widget** to jump to its event handler in code. Start a new form with **New File → Blank Form (.win)**. Hand-written `.win` files show up read-only in Design view.

### Browser
An early-web-style browser for `.rym` websites, with Back, Forward, Refresh, Home, an address bar and a status bar.
- Type a domain such as `example.rym` to open it.
- Type words without `.rym` to **search the network** instead.
- Sites are HTML/CSS only, so you get text, images and links. There are no scripts or forms.

### Terminal
A command line connected to your logged-in session. It isn't a full shell, but has most everyday commands:

```
ls [path]  cd <path>  pwd  mkdir <dir>  touch <file>
rm [-r] <path>  cp <src> <dst>  mv <src> <dst>  cat <file>
edit <file>               (blank line saves, "." cancels)
lynx <domain.rym>[/path]  search <query>
run <name>                runs /Programs/<name>.run (with input()) or opens <name>.win
publish <domain.rym>      publish the current folder as a website (see section 6)
sendfile <path> <user>    send a file to another user, sealed (see section 7)
whoami  status (username and node ID)  clear  help
```

The Terminal doesn't have email (use **Mail**), `mc` (use **File Manager**), or the full `rym` commands (use the RYMNET assistant in RAD, or a real terminal).

### Mail
Encrypted mail with no mail server, laid out like Outlook: folders (Inbox / Sent), message list, reading pane, and a **compose** window with To, Cc, Bcc, Subject and Body.
- Address mail to a **Rymnet username** (e.g. `rob`), not an email address.
- **Reply** and **Forward** fill in from the selected message.
- **Verified: yes** means the message really comes from that user: the signature matches their registered key.
- **Bcc is truly blind.** Relaying peers can't see it.
- **Attachments:** select a message with a file and use **Save as** to save it into your files.
- Limitations: **Sent** only shows what you sent in this session, and read/unread marks are only kept on this computer.
- The recipient must have logged in at least once while connected to a peer you share, so their mailbox key is published.

### Chat
IRC-style chat. Channels are on the left, the conversation and a message box on the right. Join a channel by name. **Direct Message…** sends a private, encrypted message to one user. Channel messages are signed (so you know who wrote them) but not encrypted.

### PDF Reader
Shows real PDF pages from your files, with Prev/Next, Zoom −/+, and a password field for protected PDFs. It needs the Pdfium library: if you see "Pdfium library not found", put `libpdfium.dylib` (macOS), `libpdfium.so` (Linux) or `pdfium.dll` (Windows) next to the `rymwin` program. Download it from github.com/bblanchon/pdfium-binaries. Every other app works without it.

### Paint
A simple editor for PNG images: Pencil, Eraser (makes pixels transparent), Fill, a colour picker, brush size, and up to 20 steps of **Undo**. **New** gives a blank canvas. **Save** and **Save As** write to your files.

### Writer
A Markdown editor (`.wrr` files), with the source on the left and a live preview on the right. The toolbar has Bold, Italic, Code, H1–H3, lists, quote, link and image. Undo and redo are ⌘Z / ⌘Y.
**Export as HTML** writes a complete website folder: HTML, a `style.css`, and copies of your images. You then publish that folder (section 6).

### Board
A Reddit-style forum with no voting. Type a board name on the left and press **Join**; a board is created when its first post arrives. **New Post** takes a title, body and optional link. Click a post to open it, type a comment at the bottom and press Enter, and use **Back** to return to the list. Comments are one level deep. Posts are signed but public.

---

## 6. Publishing a website (`.rym`)

1. **Make the site.** The easy way is to write it in **Writer**, then **Export as HTML** to e.g. `/sites/mysite/index.html`. Or create `index.html` (and CSS and images) yourself in File Manager or Rymcode. Use HTML and CSS only; no JavaScript.
2. **Publish it.** Open **Terminal**, `cd /sites/mysite`, then run:
   ```
   publish mysite.rym
   ```
   This claims the domain, creates the site, uploads the folder, publishes it and copies it to peers, all in one step. Running it again later updates the site.
3. **Visit it.** In RYMWIN's Browser, type `mysite.rym`.

Domains are owned by **this computer's node identity**, not by your account. Publish from the same machine each time you update a site.

From the RYMNET assistant or a real terminal, the same steps are:
`rym domain claim mysite.rym` → `rym site create mysite.rym` → `rym site upload mysite.rym <folder>` → `rym site publish mysite.rym` → `rym site replicate mysite.rym`. To make the site show up in search, run `rym index site mysite.rym`.

---

## 7. Sending files

In Terminal: `sendfile <path> <username>`. The file is encrypted so that only that user can open it, stored on your peers, and announced to them by mail. They open the message in **Mail** and use **Save as**.

The file arrives within a short time, not instantly. Both of you need at least one peer in common, and the recipient must have logged in at least once.

---

## 8. RymScript programs

RymScript is Python syntax in a safe sandbox. There are two kinds of program:

- **`.run`**: terminal programs with `print()` and `input()`. Run them with `run <name>` in Terminal.
- **`.win`**: desktop apps. Each frame the program describes its window as a list of widgets (label, button, text input, checkbox…) and reacts to an `events` dict. Run them from Rymcode or File Manager.

Programs can read and write *your* files (`read_file`, `write_file`, `append_file`, `list_dir`, `exists`, `remove_file`, `mkdir`). They can `import` `math`, `cmath`, `unicodedata`, `zlib`, `binascii`, `csv`, `struct` and `random`, use hashing (`sha256`, `md5`, `sha1`, `sha3`, `sha512`, `blake2`), and call `json_loads`/`json_dumps`. They have no network, no access to real files, and can't start other programs.

There are examples in `/Programs`: `hello.run`, `counter.win`, `checksum_tool.run` and `notes.win`. The RymScript SDK guide is the full reference.

---

## 9. How the network fits together

- **Joining:** `peer connect` only updates **your** peer list. When you add a new node to the network, also connect your older node *back* to the new one. Otherwise nodes that join later through the old node won't discover the new one.
- **What needs the daemon:** browsing `.rym` sites and search on *this* computer need a local `rymnetd` (it provides the gateway on port 7447). Accounts, mail, chat, file sync and publishing talk to your peers directly and only need peers configured. RAD's **Initiate** button starts the daemon for you.
- **Being reachable:** for other nodes to connect *to you*, port 7444 must be reachable from outside, through your router or firewall. Connecting *out* to other nodes always works.
- **Where data lives:** `~/.rymnet` holds the node identity, `config.toml`, `peers/peers.toml`, sites, the chunk store and caches.
- **Storage limits:** each node limits how much it stores for other people's accounts (100 MB by default) and mail (20 MB per recipient). The `df` command in the `rymsh` shell shows how much you're using.

---

## 10. The RYMNET assistant (chat in RAD)

On the RYMNET page in RAD, the **Assistant** panel lets you talk to Rymnet in plain language:

- **Ask how to do things.** It knows this guide. Examples: "How do I send a file to rob?", "Why does Browser say no peers configured?"
- **Have it do things for you.** It runs `rym` commands and shows each one with its output:
  - "Join the network through 139.99.155.167"
  - "Show my peers" / "ping node1" / "remove node2"
  - "Is mysite.rym taken?" / "claim mysite.rym"
  - "Publish the folder ~/Sites/mysite as mysite.rym"
  - "Search the network for recipes"
  - "Open RYMWIN" / "open rymsh" / "start the daemon"
- **Commands that remove or undo things wait for your OK**: removing peers, releasing domains, rolling back or unpinning sites, clearing the search index. Click **Run** or **Skip**. Turn on **Ask first** to approve every command.
- The assistant **can't click inside RYMWIN, type into rymsh, or see your account's files or mail**. Those stay private to your password. It works with this computer's node: peers, domains, sites, search and the daemon. It can open RYMWIN and rymsh for you and tell you what to do there.

---

## 11. Troubleshooting

| You see | What to do |
|---|---|
| "no peers configured; add one with `rym peer connect <ip>`" | In RAD → RYMNET, enter a node address and press **Initiate RYMWIN**. |
| "Rymnet isn't built yet" in RAD | Run `./scripts/build-rymnet.sh` in the RAD folder (developer builds). |
| Initiate fails at "connect to …" | Check the address and port (7444), that the other node's `rymnetd` is running, and that its firewall allows port 7444. |
| "that address is this node's own identity" | You typed your own address. Use another node's. |
| "username … already registered under a different password" | The name is taken (or you mistyped your password). Pick another name or retry carefully. |
| Browser / search show nothing or can't connect | Start the daemon: RAD → RYMNET → **Initiate RYMWIN**. |
| "no published mailbox key" when mailing someone | The recipient hasn't logged in yet while connected to a peer you share. |
| Mail or files are slow to arrive | Delivery waits for peers to sync. Make sure you and the other person share at least one peer. |
| "Pdfium library not found" | See **PDF Reader** above. |
| RYMWIN shows "RYMWIN isn't installed" | Run `./scripts/build-rymnet.sh` in the RAD folder (developer builds). |
| Dropped files didn't appear | Log in to RYMWIN first, then drop again. Look in **/Downloads** in File Manager. |
| Paste does nothing in RYMWIN | Click into a text field first. RYMWIN pastes text only, not images. |
| The daemon stopped right away | Click **Log** next to the daemon status in RAD. Usually port 7444 is already in use by another `rymnetd`. |
| Pushing your files is rejected for being over quota | Your files are over the node's storage limit. Delete files, or ask the node operator. |

---

## 12. `rym` command reference

These are what the RYMNET assistant runs. You can also type them in a real terminal.

```
rym init                                  create this node's identity (once)
rym node id | status | address | peers    node info
rym peer connect <host[:port]> [--name N] connect to a node and learn its peers
rym peer list | ping <name> | remove <name>
rym peer add <name> <public_key> <host:port>
rym domain search | claim | info | release <domain.rym>
rym domain list
rym site create | validate | publish | status | versions <domain.rym>
rym site upload <domain.rym> <folder>
rym site replicate <domain.rym> [--copies 3]
rym site fetch | pin | unpin | replicas <domain.rym>
rym site rollback <domain.rym> --version N
rym search [--network] [--domain D] <words…>
rym index status | rebuild | clear | site <domain.rym>
rym open <domain.rym>                     print the local gateway URL
rym time                                  network time check against peers
rymnetd run                               run the daemon in the foreground
```
