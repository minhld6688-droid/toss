#!/usr/bin/env bash
# Install the Toss relay (iroh-relay 1.4.1) on Ubuntu 22.04 / 24.04.
#
#   sudo ./install.sh relay.example.vn you@example.com
# or
#   sudo RELAY_DOMAIN=relay.example.vn CONTACT_EMAIL=you@example.com ./install.sh
#
# What it does (safe to re-run):
#   1. gets the iroh-relay binary: a prebuilt one if TOSS_RELAY_BIN_URL is set
#      (see .github/workflows/relay-build.yml), otherwise `cargo install
#      iroh-relay --version 1.4.1 --features server` (5-15 min on 1-2 vCPU);
#   2. creates user toss-relay, /etc/toss-relay (config), /var/lib/toss-relay (certs);
#   3. renders relay.toml (Let's Encrypt via the relay's built-in ACME);
#   4. installs + starts the systemd unit toss-relay;
#   5. opens UFW: 22/tcp (ssh), 80/tcp, 443/tcp, 7842/udp (QUIC address discovery);
#   6. waits for https://<domain>/healthz.
#
# Optional env: IROH_RELAY_VERSION (default 1.4.1), TOSS_RELAY_BIN_URL,
# TOSS_RELAY_BIN_SHA256, TOSS_RELAY_STAGING=1 (Let's Encrypt staging certs, for
# tests: browsers will not trust them), TOSS_RELAY_RATE_MBPS (default 50),
# TOSS_RELAY_SKIP_UFW=1.
set -euo pipefail

VERSION="${IROH_RELAY_VERSION:-1.4.1}"
RELAY_DOMAIN="${1:-${RELAY_DOMAIN:-}}"
CONTACT_EMAIL="${2:-${CONTACT_EMAIL:-}}"
RATE_MBPS="${TOSS_RELAY_RATE_MBPS:-50}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPT=/opt/toss-relay
CONF=/etc/toss-relay
BIN=/usr/local/bin/iroh-relay

log()  { printf '\033[1;36m[*]\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m[+]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[!]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[x]\033[0m %s\n' "$*" >&2; exit 1; }

# --- preflight ---------------------------------------------------------
[ "$(id -u)" -eq 0 ] || die "run as root: sudo $0 <domain> <email>"
if [ -z "$RELAY_DOMAIN" ] || [ -z "$CONTACT_EMAIL" ]; then die "usage: sudo $0 <relay-domain> <contact-email>"; fi
[[ "$RELAY_DOMAIN" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?\.[A-Za-z]{2,}$ ]] || die "bad domain: $RELAY_DOMAIN"
[[ "$CONTACT_EMAIL" =~ ^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+$ ]] || die "bad email: $CONTACT_EMAIL"
if ! [[ "$RATE_MBPS" =~ ^[0-9]+$ ]] || [ "$RATE_MBPS" -lt 1 ] || [ "$RATE_MBPS" -gt 4000 ]; then die "TOSS_RELAY_RATE_MBPS must be 1..4000"; fi
for f in relay.toml.in toss-relay.service update-allowed.sh check.sh; do
  [ -f "$SCRIPT_DIR/$f" ] || die "missing $SCRIPT_DIR/$f (run install.sh from the hub/relay folder)"
done

if [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  case "${ID:-}-${VERSION_ID:-}" in
    ubuntu-22.04|ubuntu-24.04) ;;
    *) warn "tested on Ubuntu 22.04/24.04, this is ${PRETTY_NAME:-unknown}; continuing" ;;
  esac
fi
command -v systemctl >/dev/null || die "systemd is required"

# DNS sanity check (warning only: Let's Encrypt needs the A record to point here).
my_ip="$(curl -4 -fsS --max-time 8 https://api.ipify.org 2>/dev/null || true)"
dns_ip="$(getent ahostsv4 "$RELAY_DOMAIN" 2>/dev/null | awk '{print $1; exit}' || true)"
if [ -z "$dns_ip" ]; then
  warn "$RELAY_DOMAIN does not resolve yet. Create the DNS A record -> ${my_ip:-the public IP of this VPS} first, certificate issuing will fail until it resolves."
elif [ -n "$my_ip" ] && [ "$dns_ip" != "$my_ip" ]; then
  warn "$RELAY_DOMAIN -> $dns_ip but the public IP of this VPS is $my_ip. Fix the A record (certificate will fail otherwise)."
else
  ok "DNS ok: $RELAY_DOMAIN -> $dns_ip"
fi

log "Installing base packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends ca-certificates curl openssl ufw build-essential pkg-config

# --- binary ------------------------------------------------------------
have_version() { [ -x "$BIN" ] && "$BIN" --version 2>/dev/null | grep -q "$VERSION"; }

if have_version; then
  ok "iroh-relay $VERSION already installed"
elif [ -n "${TOSS_RELAY_BIN_URL:-}" ]; then
  log "Downloading prebuilt iroh-relay from $TOSS_RELAY_BIN_URL"
  tmp="$(mktemp)"
  curl -fL --retry 3 -o "$tmp" "$TOSS_RELAY_BIN_URL"
  if [ -n "${TOSS_RELAY_BIN_SHA256:-}" ]; then
    echo "$TOSS_RELAY_BIN_SHA256  $tmp" | sha256sum -c - || die "sha256 mismatch for the downloaded binary"
  else
    warn "TOSS_RELAY_BIN_SHA256 not set: binary not verified"
  fi
  install -m 0755 "$tmp" "$BIN"
  rm -f "$tmp"
  have_version || die "downloaded binary does not report version $VERSION"
else
  log "Building iroh-relay $VERSION from source (this takes 5-15 minutes)"
  mem_kb="$(awk '/MemTotal/{print $2}' /proc/meminfo)"
  swapfile=""
  if [ "$mem_kb" -lt 2000000 ] && [ "$(swapon --show --noheadings | wc -l)" -eq 0 ]; then
    log "Less than 2 GB RAM and no swap: adding a temporary 2 GB swap file for the build"
    swapfile=/swapfile.toss-build
    fallocate -l 2G "$swapfile" 2>/dev/null || dd if=/dev/zero of="$swapfile" bs=1M count=2048 status=none
    chmod 600 "$swapfile"
    mkswap "$swapfile" >/dev/null
    swapon "$swapfile"
  fi
  export RUSTUP_HOME="$OPT/rustup" CARGO_HOME="$OPT/cargo"
  mkdir -p "$OPT"
  if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    log "Installing Rust (rustup, minimal profile): iroh-relay needs Rust >= 1.91"
    curl -fsS --proto '=https' --tlsv1.2 https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
  else
    "$CARGO_HOME/bin/rustup" update stable --no-self-update >/dev/null 2>&1 || true
  fi
  if [ "$mem_kb" -lt 2000000 ]; then export CARGO_BUILD_JOBS=1; fi
  "$CARGO_HOME/bin/cargo" install iroh-relay --version "$VERSION" --features server --locked --root "$OPT/build" --force
  install -m 0755 "$OPT/build/bin/iroh-relay" "$BIN"
  if [ -n "$swapfile" ]; then swapoff "$swapfile" && rm -f "$swapfile"; fi
  have_version || die "built binary does not report version $VERSION"
fi
ok "$("$BIN" --version)"

# --- user, dirs, files ---------------------------------------------------
id -u toss-relay >/dev/null 2>&1 || useradd --system --home-dir /var/lib/toss-relay --shell /usr/sbin/nologin toss-relay
install -d -m 0755 "$OPT"
install -d -m 0750 -o root -g toss-relay "$CONF"
install -m 0644 "$SCRIPT_DIR/relay.toml.in" "$OPT/relay.toml.in"
install -m 0755 "$SCRIPT_DIR/update-allowed.sh" "$OPT/update-allowed.sh"
install -m 0755 "$SCRIPT_DIR/check.sh" "$OPT/check.sh"
[ -f "$CONF/allowed.txt" ] || install -m 0640 -o root -g toss-relay "$SCRIPT_DIR/allowed.txt.example" "$CONF/allowed.txt"

prod_tls=true
if [ "${TOSS_RELAY_STAGING:-0}" = "1" ]; then
  prod_tls=false
  warn "Using Let's Encrypt STAGING: certificates are not trusted by clients. Re-run without TOSS_RELAY_STAGING for production."
fi
cat > "$CONF/relay.env" <<EOF
RELAY_DOMAIN=$RELAY_DOMAIN
CONTACT_EMAIL=$CONTACT_EMAIL
PROD_TLS=$prod_tls
RATE_BPS=$((RATE_MBPS * 1024 * 1024))
EOF
chmod 640 "$CONF/relay.env"
chgrp toss-relay "$CONF/relay.env"

install -m 0644 "$SCRIPT_DIR/toss-relay.service" /etc/systemd/system/toss-relay.service
systemctl daemon-reload
"$OPT/update-allowed.sh" --no-restart

# --- firewall ------------------------------------------------------------
if [ "${TOSS_RELAY_SKIP_UFW:-0}" = "1" ]; then
  warn "Skipping UFW (TOSS_RELAY_SKIP_UFW=1). Open 80/tcp, 443/tcp, 7842/udp yourself."
else
  log "Configuring UFW"
  ssh_port="$( (sshd -T 2>/dev/null || true) | awk '/^port /{print $2; exit}')"
  ufw allow "${ssh_port:-22}/tcp" comment 'ssh' >/dev/null
  ufw allow 80/tcp   comment 'toss-relay http'  >/dev/null
  ufw allow 443/tcp  comment 'toss-relay https' >/dev/null
  ufw allow 7842/udp comment 'toss-relay QUIC address discovery' >/dev/null
  ufw --force enable >/dev/null
  ufw status | sed 's/^/    /'
fi

# --- start ---------------------------------------------------------------
log "Starting toss-relay"
systemctl enable toss-relay >/dev/null 2>&1
systemctl restart toss-relay

url="https://$RELAY_DOMAIN"
log "Waiting for $url/healthz (Let's Encrypt can take up to ~2 minutes)"
up=0
for _ in $(seq 1 40); do
  if ! systemctl is-active --quiet toss-relay; then break; fi
  if [ "$prod_tls" = "true" ] && curl -fsS --max-time 5 -o /dev/null "$url/healthz" 2>/dev/null; then up=1; break; fi
  if [ "$prod_tls" = "false" ] && curl -fsSk --max-time 5 -o /dev/null "$url/healthz" 2>/dev/null; then up=1; break; fi
  sleep 3
done

if [ "$up" -eq 1 ]; then
  ok "Relay is up: $url"
  echo
  echo "  Paste this into Toss on the studio host AND on every editor:"
  echo "      Cai dat -> Ket noi -> Nang cao -> Relay rieng:   $url"
  echo
  echo "  Check any time:   $OPT/check.sh $RELAY_DOMAIN"
  echo "  Logs:             journalctl -u toss-relay -f"
  echo "  Restrict access:  edit $CONF/allowed.txt, then $OPT/update-allowed.sh"
else
  warn "The relay did not answer on $url/healthz yet. Last log lines:"
  journalctl -u toss-relay -n 25 --no-pager >&2 || true
  cat >&2 <<EOT

Common causes:
  - DNS A record for $RELAY_DOMAIN missing or not propagated yet (wait, then: systemctl restart toss-relay)
  - the VPS provider's own firewall / security group blocks 443/tcp (open 80, 443 tcp and 7842 udp in the panel)
  - Let's Encrypt rate limit (try TOSS_RELAY_STAGING=1 first when testing)
EOT
  exit 1
fi
