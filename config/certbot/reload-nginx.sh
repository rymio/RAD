#!/bin/sh
# /etc/letsencrypt/renewal-hooks/deploy/reload-nginx.sh
# Reloads nginx after certificate renewal. RAD itself keeps running — the vault stays unlocked.
set -e
nginx -t && systemctl reload nginx
