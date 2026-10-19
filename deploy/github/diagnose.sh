#!/bin/sh
# Read-only host report. The runner feeds this script to `bash -s` on stdin.
set -u
PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

section() {
  printf '\n===== %s =====\n' "$1"
}

doctor_rc=0
section 'kiln-runtime doctor'
if [ -x /opt/kiln/bin/kiln-runtime ]; then
  kiln-runtime doctor || doctor_rc=$?
else
  printf 'kiln-runtime is not installed\n'
  doctor_rc=1
fi

section 'firecracker version'
firecracker --version || printf 'firecracker exit %s\n' "$?"
jailer --version || printf 'jailer exit %s\n' "$?"

section 'cgroup type'
stat -fc '%T' /sys/fs/cgroup || printf 'stat exit %s\n' "$?"
if [ -r /sys/fs/cgroup/cgroup.controllers ]; then
  printf 'cgroup.controllers: '
  cat /sys/fs/cgroup/cgroup.controllers
fi

for unit in kiln-cgroup.service kiln-host.service kiln-api.service kiln-network.service; do
  section "systemctl status ${unit}"
  systemctl status "$unit" --no-pager -n 20 || printf 'status exit %s\n' "$?"
  section "journalctl ${unit}"
  journalctl -u "$unit" -n 200 --no-pager || printf 'journal exit %s\n' "$?"
done

exit "$doctor_rc"
