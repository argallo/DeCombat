#!/usr/bin/env bash
# ci_vps_ssh.sh: the workflow's ssh setup for the VPS, host `dc-vps` in ~/.ssh/config, the host key pinned from a secret
# (deploy-servers.yml). Reads DC_VPS_HOST, DC_VPS_USER (optional, default root), DC_VPS_SSH_KEY, DC_VPS_KNOWN_HOSTS.
set -euo pipefail
for v in DC_VPS_HOST DC_VPS_SSH_KEY DC_VPS_KNOWN_HOSTS; do
  [ -n "${!v:-}" ] || { echo "::error::the secret $v is not set (docs/RELEASES.md)"; exit 1; }
done
mkdir -p ~/.ssh && chmod 700 ~/.ssh
printf '%s\n' "$DC_VPS_SSH_KEY" > ~/.ssh/dc_vps_key && chmod 600 ~/.ssh/dc_vps_key
printf '%s\n' "$DC_VPS_KNOWN_HOSTS" > ~/.ssh/dc_vps_known_hosts && chmod 600 ~/.ssh/dc_vps_known_hosts
cat >> ~/.ssh/config <<CONF
Host dc-vps
  HostName $DC_VPS_HOST
  User ${DC_VPS_USER:-root}
  IdentityFile ~/.ssh/dc_vps_key
  IdentitiesOnly yes
  BatchMode yes
  ConnectTimeout 20
  StrictHostKeyChecking yes
  UserKnownHostsFile ~/.ssh/dc_vps_known_hosts
CONF
chmod 600 ~/.ssh/config
