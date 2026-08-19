# Sourced by the check scripts. Signs a candidate in through the same door the
# browser uses, so no check needs sqlite3, the cookie secret, or its own copy of
# the signing scheme, and every one of them works against a server it did not
# start.
#
# Usage: cookie=$(login_session_cookie "$BASE_URL" some-handle) || exit 2

login_session_cookie() {
  base_url=$1
  handle=$2
  cookie=$(curl -sS -X POST \
    -H "Content-Type: application/json" \
    -d "{\"login\":\"$handle\"}" \
    -D - -o /dev/null "$base_url/api/login" \
    | grep -i '^set-cookie:' \
    | sed -n 's/.*\(codetrial_session=[^;]*\).*/\1/p')
  if [ -z "$cookie" ]; then
    echo "$base_url/api/login did not return a session cookie." >&2
    return 1
  fi
  printf '%s\n' "$cookie"
}
