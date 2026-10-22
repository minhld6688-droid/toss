#!/usr/bin/env bash
# Regenerate /etc/toss-relay/relay.toml from the template + relay.env + allowed.txt,
# then restart the relay.  Run after editing allowed.txt or relay.env.
#
#   sudo /opt/toss-relay/update-allowed.sh [--no-restart]
#
# allowed.txt: one endpoint id (64 hex chars) per line, '#' starts a comment.
# No ids = relay stays open to everyone (access = "everyone").
set -euo pipefail

CONF_DIR="${TOSS_RELAY_CONF_DIR:-/etc/toss-relay}"
TEMPLATE="${TOSS_RELAY_TEMPLATE:-/opt/toss-relay/relay.toml.in}"
ENV_FILE="$CONF_DIR/relay.env"
ALLOWED="$CONF_DIR/allowed.txt"
OUT="$CONF_DIR/relay.toml"
RESTART=1
[ "${1:-}" = "--no-restart" ] && RESTART=0

die() { echo "update-allowed: $*" >&2; exit 1; }

[ -r "$TEMPLATE" ] || die "template not found: $TEMPLATE"
[ -r "$ENV_FILE" ] || die "missing $ENV_FILE (run install.sh first)"
# shellcheck disable=SC1090
. "$ENV_FILE"
: "${RELAY_DOMAIN:?RELAY_DOMAIN missing in $ENV_FILE}"
: "${CONTACT_EMAIL:?CONTACT_EMAIL missing in $ENV_FILE}"
PROD_TLS="${PROD_TLS:-true}"
RATE_BPS="${RATE_BPS:-52428800}"

[[ "$RELAY_DOMAIN" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]] || die "bad RELAY_DOMAIN: $RELAY_DOMAIN"
[[ "$CONTACT_EMAIL" =~ ^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+$ ]] || die "bad CONTACT_EMAIL: $CONTACT_EMAIL"
[[ "$PROD_TLS" =~ ^(true|false)$ ]] || die "PROD_TLS must be true or false"
if ! [[ "$RATE_BPS" =~ ^[0-9]+$ ]] || [ "$RATE_BPS" -lt 1 ] || [ "$RATE_BPS" -gt 4294967295 ]; then die "RATE_BPS must be 1..4294967295"; fi

# --- allow-list ---------------------------------------------------------
ids=()
if [ -f "$ALLOWED" ]; then
  n=0
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    line="${line%%#*}"                       # strip comment
    line="$(echo "$line" | tr -d '[:space:]' | tr 'A-F' 'a-f')"
    [ -z "$line" ] && continue
    [[ "$line" =~ ^[0-9a-f]{64}$ ]] || die "$ALLOWED line $n: not a 64-char hex endpoint id"
    ids+=("$line")
  done < "$ALLOWED"
fi

if [ "${#ids[@]}" -eq 0 ]; then
  ACCESS='access = "everyone"'
  MODE="OPEN (no allowed.txt entries)"
else
  mapfile -t ids < <(printf '%s\n' "${ids[@]}" | sort -u)
  list=""
  for id in "${ids[@]}"; do list+="  \"$id\","$'\n'; done
  ACCESS="access.allowlist = ["$'\n'"${list}]"
  MODE="ALLOW-LIST (${#ids[@]} devices)"
fi

# --- render -------------------------------------------------------------
t="$(cat "$TEMPLATE")"
t="${t//@ACCESS@/$ACCESS}"
t="${t//@DOMAIN@/$RELAY_DOMAIN}"
t="${t//@CONTACT_EMAIL@/$CONTACT_EMAIL}"
t="${t//@PROD_TLS@/$PROD_TLS}"
t="${t//@RATE_BPS@/$RATE_BPS}"
case "$t" in *@[A-Z_]*@*) die "unreplaced placeholder left in template" ;; esac

tmp="$(mktemp "$CONF_DIR/relay.toml.XXXXXX")"
printf '%s\n' "$t" > "$tmp"
chmod 640 "$tmp"
chgrp toss-relay "$tmp" 2>/dev/null || true
[ -f "$OUT" ] && cp -p "$OUT" "$OUT.bak"
mv "$tmp" "$OUT"
echo "relay.toml written: access = $MODE, domain = $RELAY_DOMAIN"

# --- restart ------------------------------------------------------------
if [ "$RESTART" -eq 1 ]; then
  systemctl restart toss-relay
  sleep 3
  if ! systemctl is-active --quiet toss-relay; then
    echo "update-allowed: service failed to start with the new config:" >&2
    journalctl -u toss-relay -n 15 --no-pager >&2 || true
    if [ -f "$OUT.bak" ]; then
      cp -p "$OUT.bak" "$OUT"
      systemctl restart toss-relay || true
      echo "update-allowed: previous relay.toml restored" >&2
    fi
    exit 1
  fi
  echo "toss-relay restarted OK"
fi
