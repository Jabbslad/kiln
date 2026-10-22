#!/bin/sh
# deploy/github/ssh-session.sh and ssh.sh share one ControlPath and retry only pre-auth failures.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() {
  printf 'test-ssh-multiplex: %s\n' "$*" >&2
  exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

log=$work/invocations.log
: >"$log"
master_opens=$work/master-opens
: >"$master_opens"
kex_fails=$work/kex-fails
printf '2\n' >"$kex_fails"

cat >"$work/ssh" <<EOF
#!/bin/sh
printf '%s\n' "ssh \$*" >> '$log'
for arg in "\$@"; do
  case "\$arg" in
    ControlPath=*) path=\${arg#ControlPath=} ;;
  esac
done
case " \$* " in
  *" -O exit "*)
    exit 0
    ;;
esac
case " \$* " in
  *ControlMaster=yes*|*" -M "*)
    [ -n "\$path" ] || exit 97
    printf '%s\n' "\$path" >> '$master_opens'
    n=\$(cat '$kex_fails')
    if [ "\$n" -gt 0 ]; then
      printf '%s\n' \$((n - 1)) > '$kex_fails'
      printf 'kex_exchange_identification: read: Connection reset by peer\n' >&2
      exit 255
    fi
    exit 0
    ;;
esac
while [ \$# -gt 0 ]; do
  if [ "\$1" = -- ]; then
    shift
    "\$@"
    exit \$?
  fi
  shift
done
exit 0
EOF

cat >"$work/scp" <<EOF
#!/bin/sh
printf '%s\n' "scp \$*" >> '$log'
for arg in "\$@"; do
  case "\$arg" in
    ControlPath=*) path=\${arg#ControlPath=} ;;
  esac
done
[ -n "\$path" ] || exit 96
exit 0
EOF
chmod 755 "$work/ssh" "$work/scp"

key='-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIGfakekeyfakekeyfakekeyfakekeyfakekeyfa
-----END PRIVATE KEY-----'
hosts='dark-forge.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHRlc3Qta25vbi1ob3N0LWtleS1waW4='

export PATH="$work:$PATH"
export DARK_FORGE_SSH_KEY="$key"
export DARK_FORGE_KNOWN_HOSTS="$hosts"
export KILN_SSH_CONTROL_PATH="$work/kiln-ssh-control"
export KILN_SSH_SH="$root/deploy/github/ssh.sh"

bash "$root/deploy/github/ssh-session.sh" open \
  || fail 'master open did not succeed after kex retries'
[ "$(wc -l <"$master_opens" | tr -d ' ')" = 3 ] \
  || fail 'expected three master open attempts after two kex failures'

bash "$root/deploy/github/ssh.sh" scp "$work/ssh" /tmp/a
bash "$root/deploy/github/ssh.sh" scp "$work/scp" /tmp/b
bash "$root/deploy/github/ssh.sh" ssh -- printf ok

paths=$(
  sed -n 's/.*ControlPath=\([^ ]*\).*/\1/p' "$log" | sort -u
)
printf '%s\n' "$paths" | grep -qx "$KILN_SSH_CONTROL_PATH" \
  || fail 'ssh/scp calls did not share one ControlPath'
[ "$(printf '%s\n' "$paths" | grep -c .)" = 1 ] \
  || fail 'multiple ControlPath values in log'

bash "$root/deploy/github/ssh.sh" ssh -- false 2>"$work/remote.err" \
  && fail 'remote false should fail'
grep -c ' false$' "$log" | grep -qx 1 \
  || fail 'remote command failure was retried'

bash "$root/deploy/github/ssh-session.sh" close
grep -q 'ssh -O exit' "$log" || fail 'master was not closed with ssh -O exit'

printf 'test-ssh-multiplex ok\n'
