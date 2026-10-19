# Guest access

`kiln ssh`, `kiln cp`, and `kiln ssh-config` open a guest shell or copy a file. Traffic is HTTPS to the host and vsock inside the guest. There is no public SSH port. The commands need a matching Ubuntu guest with OpenSSH. Windows builds are management-only.

## Sub-features

- `kiln ssh BOX` for a shell, or `kiln ssh BOX -- COMMAND` for one command. The client execs the system `ssh`.
- `kiln cp LOCAL BOX:/absolute/path` and the reverse. Exactly one side uses `BOX:/absolute/path`. The client execs `scp`.
- `kiln ssh-config BOX` prints a `Host` block. The alias is `kiln-PROFILE-BOXID`. `ProxyCommand` runs `kiln ssh-proxy`.
- `kiln ssh-proxy` is hidden. OpenSSH invokes it. It upgrades the HTTPS connection with `kiln-ssh`.

## How to get to it (user POV)

Install OpenSSH on the laptop. Log in or add a direct profile. The box is a running Ubuntu guest built with the current agent, not the BusyBox fixture. Then:

```sh
kiln ssh BOX -- printf auth-ssh-ok
kiln cp ./note.txt BOX:/root/note.txt
kiln ssh-config BOX
```

`scripts/test-ssh-pty.py SSH_CONFIG HOST` drives an interactive session that already has an SSH config: it checks `stty size` is `31 97` and that the guest does not print `System is booting up`.

## Driving it with tmux

```sh
tmux new-session -d -s kiln-ssh -c "$PWD" -- "${SHELL:-bash}" -l
tmux send-keys -t kiln-ssh 'kiln --config "$KILN_CONFIG" ssh "$BOX_ID" -- printf auth-ssh-ok; echo EXIT:$?' C-m
tmux capture-pane -p -t kiln-ssh
```

A pass prints `auth-ssh-ok` and `EXIT:0`. For a failing guest command the client returns the remote status (`scripts/test-central-login.py` expects exit 37). `kiln cp` is a pass when the destination file's bytes match the source and `scp` exits 0.

`ssh-config` is a pass when the printed block contains `ProxyCommand`, `IdentitiesOnly yes`, and `GlobalKnownHostsFile /dev/null`, and the identity file mode is owner-only.

## Gotchas

- The BusyBox fixture has no `sshd`. `images/build-ubuntu.sh` checks `usr/sbin/sshd` and related paths exist in the root tarball. It does not check `/sbin/ip` (see `guest-networking.md`).
- Guest images and templates are not upgraded by installing a new client or host binary. An old disk can still print the PAM `System is booting up` warning.
- SSH and SFTP need the box `running`. A stopped or paused box produces the start/resume hint and does not change the box.
- `scripts/test-central-login.py` refuses a direct administrator profile. It does not perform login.
- This journey was not driven in the authoring proof. It needs KVM, an Ubuntu image, and a reachable API. CI does not run `scripts/test-network-kvm.sh` or the ignored lifecycle tests (`.github/workflows/build.yml`).
