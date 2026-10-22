#!/usr/bin/env bash
# Quick health check of the Toss relay. Works on the VPS and on any Mac/Linux
# machine (needs bash, curl, openssl).
#
#   ./check.sh relay.example.vn
#
# Exit code 0 = everything the check can see from outside is fine.
set -uo pipefail

domain="${1:-}"
domain="${domain#https://}"
domain="${domain%%/*}"
[ -n "$domain" ] || { echo "usage: $0 <relay-domain>   (e.g. relay.example.vn)"; exit 2; }

bad=0
pass() { printf '  [ok]   %s\n' "$*"; }
fail() { printf '  [FAIL] %s\n' "$*"; bad=1; }

echo "Relay check: https://$domain"

ip="$( (getent ahostsv4 "$domain" 2>/dev/null || true) | awk '{print $1; exit}')"
if [ -z "$ip" ] && command -v dscacheutil >/dev/null 2>&1; then
  ip="$(dscacheutil -q host -a name "$domain" 2>/dev/null | awk '/ip_address/{print $2; exit}')"
fi
if [ -n "$ip" ]; then pass "DNS: $domain -> $ip"; else fail "DNS: $domain does not resolve"; fi

err="$(mktemp)"
trap 'rm -f "$err"' EXIT
code="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "https://$domain/healthz" 2>"$err")"
if [ "$code" = "200" ]; then pass "HTTPS /healthz = 200 (valid certificate)"; else fail "HTTPS /healthz = ${code:-none} $(tr '\n' ' ' <"$err")"; fi

code="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "https://$domain/generate_204" 2>/dev/null)"
if [ "$code" = "204" ]; then pass "HTTPS /generate_204 = 204 (latency probe)"; else fail "HTTPS /generate_204 = ${code:-none}"; fi

if command -v openssl >/dev/null 2>&1; then
  end="$(echo | openssl s_client -servername "$domain" -connect "$domain:443" 2>/dev/null | openssl x509 -noout -enddate 2>/dev/null | cut -d= -f2)"
  if [ -n "$end" ]; then pass "Certificate valid until: $end"; else fail "could not read the TLS certificate"; fi
fi

# The relay's UDP 7842 (QUIC address discovery) cannot be probed with plain tools.
if [ -d /run/systemd/system ] && systemctl list-unit-files toss-relay.service >/dev/null 2>&1; then
  if systemctl is-active --quiet toss-relay; then pass "service toss-relay is active"; else fail "service toss-relay is NOT active"; fi
  if command -v ss >/dev/null 2>&1; then
    if ss -ulnH 2>/dev/null | awk '{print $4}' | grep -q ':7842$'; then pass "UDP 7842 is listening"; else fail "UDP 7842 is not listening"; fi
  fi
  m="$(curl -fsS --max-time 3 http://127.0.0.1:9090/metrics 2>/dev/null || true)"
  if [ -n "$m" ]; then
    acc="$(printf '%s\n' "$m" | awk '/^relayserver_accepts/{print $2; exit}')"
    dis="$(printf '%s\n' "$m" | awk '/^relayserver_disconnects/{print $2; exit}')"
    sent="$(printf '%s\n' "$m" | awk '/^relayserver_bytes_sent/{print $2; exit}')"
    echo "  [info] connections accepted ${acc:-?}, disconnected ${dis:-?}, bytes relayed to clients ${sent:-?}"
  fi
else
  echo "  [info] UDP 7842: confirm in your VPS panel that it is open (cannot be tested from here)"
fi

[ "$bad" -eq 0 ] && echo "ALL OK" || echo "PROBLEMS FOUND"
exit "$bad"
