# Deploying RAD on Ubuntu with nginx + systemd

```
 RAD apps ──HTTPS:443──▶ nginx (TLS, Let's Encrypt) ──HTTP──▶ 127.0.0.1:8765 rad-server ──▶ /var/lib/rad/personal.radvault
```

| File | Install to |
|---|---|
| `systemd/rad-server.service` | `/etc/systemd/system/rad-server.service` |
| `systemd/rad-server.env` | `/etc/rad/rad-server.env` |
| `nginx/rad.conf` | `/etc/nginx/sites-available/rad.conf` (+ symlink in `sites-enabled`) |
| `nginx/rad-proxy.conf` | `/etc/nginx/snippets/rad-proxy.conf` |
| `nginx/rad-ratelimit.conf` | `/etc/nginx/conf.d/rad-ratelimit.conf` |
| `certbot/reload-nginx.sh` | `/etc/letsencrypt/renewal-hooks/deploy/reload-nginx.sh` |

Target: Ubuntu 22.04 / 24.04 (written for these releases; not yet run on a live server). You need a DNS name (below: `rad.example.com`) pointing at the server, with ports 80 and 443 open.

## 1. Build and install rad-server

```bash
sudo apt update
sudo apt install -y build-essential pkg-config perl git curl ffmpeg nginx certbot
curl https://sh.rustup.rs -sSf | sh -s -- -y && . ~/.cargo/env

sudo git clone <your-repo-url> /opt/rad && sudo chown -R $USER /opt/rad   # or rsync from your Mac
cd /opt/rad
cargo build --release -p rad-server            # first build ~5–10 min
sudo install -m 755 target/release/rad-server /usr/local/bin/rad-server
```

## 2. Service user, data folder, systemd

```bash
sudo useradd --system --home /var/lib/rad --shell /usr/sbin/nologin rad
sudo install -d -o rad -g rad -m 700 /var/lib/rad /var/lib/rad/backups
sudo install -d -m 755 /etc/rad
sudo install -m 600 config/systemd/rad-server.env /etc/rad/rad-server.env
sudo cp config/systemd/rad-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rad-server
journalctl -u rad-server -n 40 --no-pager       # note the pairing code
```

## 3. HTTPS certificate

```bash
DOMAIN=rad.example.com
sudo mkdir -p /var/www/letsencrypt
# Temporary HTTP-only site so Let's Encrypt can verify the domain:
echo "server { listen 80; server_name $DOMAIN; location /.well-known/acme-challenge/ { root /var/www/letsencrypt; } }" \
  | sudo tee /etc/nginx/sites-available/rad-acme.conf >/dev/null
sudo ln -sf /etc/nginx/sites-available/rad-acme.conf /etc/nginx/sites-enabled/
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx
sudo certbot certonly --webroot -w /var/www/letsencrypt -d $DOMAIN --agree-tos -m you@example.com -n
sudo rm /etc/nginx/sites-enabled/rad-acme.conf
```

## 4. nginx site

```bash
sudo cp config/nginx/rad-proxy.conf     /etc/nginx/snippets/rad-proxy.conf
sudo cp config/nginx/rad-ratelimit.conf /etc/nginx/conf.d/rad-ratelimit.conf
sudo sed "s/rad.example.com/$DOMAIN/g" config/nginx/rad.conf | sudo tee /etc/nginx/sites-available/rad.conf >/dev/null
sudo ln -sf /etc/nginx/sites-available/rad.conf /etc/nginx/sites-enabled/rad.conf
sudo install -m 755 config/certbot/reload-nginx.sh /etc/letsencrypt/renewal-hooks/deploy/reload-nginx.sh
sudo nginx -t && sudo systemctl reload nginx

sudo ufw allow OpenSSH && sudo ufw allow 'Nginx Full' && sudo ufw enable   # 8765 stays closed
curl -s https://$DOMAIN/v1/status                                           # → {"name":"RAD server",…}
```

## 5. Connect the apps

1. Open RAD on your computer.
2. Enter the server address `https://rad.example.com`.
3. Enter the pairing code from step 2.
4. Create your passphrase and **save the recovery kit**.

For more devices, create pairing codes in **Settings → Devices**.

## Day-to-day

| Task | Command |
|---|---|
| Logs | `journalctl -u rad-server -f` |
| New pairing code | In the app: **Settings → Devices** (or restart the service; a code is printed at startup) |
| Update | `cd /opt/rad && git pull && cargo build --release -p rad-server && sudo install -m 755 target/release/rad-server /usr/local/bin/ && sudo systemctl restart rad-server` |
| Backups | Encrypted copies in `/var/lib/rad/backups`. Copy them off-site, e.g. `rclone copy /var/lib/rad/backups b2:my-rad-backups` |
| Verify vault | `sudo systemctl stop rad-server && sudo -u rad RAD_VAULT=/var/lib/rad/personal.radvault rad-server verify && sudo systemctl start rad-server` |

**After every reboot or restart the vault is locked.** Open the RAD app and enter your passphrase; indexing then resumes. Stopping the service locks the vault cleanly first, because the unit sends SIGINT.

## Why the config looks like this

- **`proxy_request_buffering off` and `proxy_buffering off`, `proxy_max_temp_file_size 0`.** By default nginx spools uploads and large responses to temp files on disk, and those would be **decrypted plaintext**. These settings stream everything straight through, so nothing readable is left on disk.
- **Long timeouts, no gzip, and `X-Accel-Buffering: no` on `/v1/events` and chat routes.** These are Server-Sent Event streams (live sync and streamed answers). Buffering would freeze them.
- **`client_max_body_size 8g`.** This must match `RAD_MAX_UPLOAD_MB`.
- **`rad-server` listens only on `127.0.0.1`.** Port 8765 is never exposed; only nginx can reach it.
- **systemd hardening.**
  - Read-only system with write access only to `/var/lib/rad`.
  - No capabilities and no privilege escalation.
  - `LimitCORE=0` and `MemorySwapMax=0` keep the in-memory vault key out of core dumps and swap. `MemorySwapMax` needs cgroup v2, the default on Ubuntu 22.04+.
- **Certificate renewal only reloads nginx.** RAD keeps running, so the vault stays unlocked.

### Prefer no public domain?

Skip steps 3–4 and use Tailscale instead:

```bash
sudo tailscale serve --bg --https=443 http://127.0.0.1:8765
```

Then connect to `https://<server>.<tailnet>.ts.net`.
