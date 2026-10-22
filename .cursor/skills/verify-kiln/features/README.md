# Kiln feature map

User-facing surfaces on current `main`. Each file is one journey. KVM means a real Firecracker guest; the dashboard preview and the installer harness do not boot one. The public client ships for Linux and macOS only; Windows builds were dropped (#110).

| Feature | File | What the user does | KVM |
| --- | --- | --- | --- |
| PR proof capture | [proof.md](proof.md) | Run `scripts/proof.sh` before/after UI or CLI output changes and attach PNGs or video to the PR | No |
| Dashboard | [dashboard.md](dashboard.md) | Preview inventory or sign in and drive live mutations, exec, and `GET /api/v1/dashboard` | No for preview; live mutations need an enrolled host |
| Auto-release | [auto-release.md](auto-release.md) | Follow a green main push through release-plan, publish, and dark-forge deploy | No |
| Boxes | [boxes.md](boxes.md) | Create, list, inspect, pause, resume, stop, start, delete | Yes for a guest |
| Exec | [exec.md](exec.md) | Run a command in a box and read its stdout and exit code | Yes |
| Guest access | [guest-access.md](guest-access.md) | `kiln ssh`, `kiln cp`, `kiln ssh-config` | Yes, plus OpenSSH and an Ubuntu guest |
| Templates and checkpoints | [templates-and-checkpoints.md](templates-and-checkpoints.md) | Build a template, clone it, save and restore a checkpoint | Yes |
| Guest networking | [guest-networking.md](guest-networking.md) | Opt-in isolated IPv4 egress | Yes, plus root and `kiln-network` |
| Login | [login.md](login.md) | `kiln login` device-code approval | No, but the real identity origin and a provider account |
| Public door | [public-door.md](public-door.md) | Laptop login through `cli.dark-forge.dev`, then list, create, exec, ssh, delete | Yes for the guest. The lab stack uses a stand-in provider |
| Private CA | [private-ca.md](private-ca.md) | `kiln profile add` with a server CA, then talk to the API | No for an empty catalog |
| Installer | [installer.md](installer.md) | `install.sh` client or `install.sh server` | Server install yes; the in-repo harness no |
