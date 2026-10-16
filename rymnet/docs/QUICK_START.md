Rymnet Quick Start
Node 1 (the bootstrap node)

139.99.155.167
rym init
nohup rymnetd run > rymnetd.log 2>&1 &
Note its IP (hostname -I on Linux, or whatever address other machines can reach it on) — call it NODE1_IP.

Node 2

202.91.202.179

rym init
nohup rymnetd run > rymnetd.log 2>&1 &
rym peer connect NODE1_IP:7444 --name node1
Then, back on Node 1, connect it back to Node 2 (this is the one step that matters for growing the mesh — see note below):


rym peer connect NODE2_IP:7444 --name node2
Node 3


rym init
nohup rymnetd run > rymnetd.log 2>&1 &
rym peer connect NODE1_IP:7444 --name node1
That's it — since Node 1 already knows about Node 2 (from the step above), this single connect call to Node 1 automatically discovers, independently verifies, and adds Node 2 as well. Node 3 ends up peered with both.

First user connects (on any node with rymnetd running and at least one peer)


rymsh
rymsh:/home$ login alice a-strong-password
That's the whole thing — login derives the account, registers the username, and pulls/pushes over whatever peers that node has configured.

Why the "connect Node 1 back to Node 2" step matters: peer connect is one-directional — it only updates your own peers.toml. If Node 2 connects to Node 1 but Node 1 never connects back, Node 1 never learns Node 2 exists, so Node 3 connecting to Node 1 would only discover Node 1, not Node 2. Doing that one reverse connect after each new node joins is what turns "connect to Node 1" into "join the whole known mesh" for everyone who bootstraps through it afterward.

Want me to add this as a "Quick Start" section at the top of INSTALL_AND_CLI_GUIDE.md

?

user

# 1. Create this laptop's own node identity (no server starts from this)
rym init

# 2. Connect to one of your two existing nodes -- since they're already
#    peered with each other, this pulls in both in one step
rym peer connect 139.99.155.167:7444 --name node1

# 3. Confirm you're actually linked to both
rym peer list
rym peer ping node1
rym peer ping <name of the auto-discovered second node from peer list>
Then just run the shell:


rymsh
You'll see:


no rymnet daemon detected at 127.0.0.1:7447; file commands still work, but
browsing and account sync need `rymnetd run`
That message is a bit imprecise for your setup — ignore the "account sync" part, it's wrong there; only the browsing part is true for you. Log in as normal:


rymsh:/home$ login alice a-strong-password
login, sync/push/pull, mc, python, emailto/email, df, publish — all of that talks directly to your two configured peers over the peer protocol (port 7444) and works fine with zero local daemon.

What won't work on this laptop: lynx, search, and the search command — those need a local HTTP gateway (port 7447), which is hard-restricted to loopback and only exists if rymnetd run is running somewhere on this same machine. If you want browsing too, run rymnetd run locally in the background (even with the same two remote peers configured) just to get that local gateway — it doesn't need to host anything or be reachable from outside.