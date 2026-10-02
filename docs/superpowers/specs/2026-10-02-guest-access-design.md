# Guest access and internet egress

Approved in this thread on 2026-10-02. The owner subsequently authorized commit,
push and release after each iteration (see `AGENTS.md`). Deployment to the running
installation still requires separate authorization.

Use OpenSSH on the laptop and in the Ubuntu guest. The existing authenticated
HTTPS gateway tunnels a bounded byte stream through the privileged host service
and Firecracker vsock. No SSH listener is exposed on the server network. SSH
provides interactive terminals, resize/signals, SFTP and editor forwarding.
The client obtains the guest host public key over verified HTTPS and pins it;
neither insecure host-key checking nor password login is permitted. Client keys
are private local files; terminal traffic is not recorded in the operation journal.

Guest SSH is unavailable before the initialization barrier. Host keys are made
after cloning, never in a reusable template. Sessions have bounded concurrency;
stop/delete/restart cannot silently attach an old connection to another box.
Management requests remain independent of streaming session capacity.

Internet access requires network-enabled templates, explicitly configured by the
operator. Add per-box interfaces and host-enforced IPv4 egress with no unsolicited
inbound access. Block other guests, host management, private LANs and metadata
endpoints. No IPv6 bypass. Old templates remain networkless and existing boxes
and disks are not replaced. Do not modify shared routing/firewall state while
implementing or running ordinary tests.

Verify protocol/authentication and stream bounds, SSH host-key checking, binary
file transfer, terminal behavior and cancellation, lifecycle interactions, and
egress isolation. Run privileged checks only in disposable, explicitly scoped
environments. Report separately what is implemented, tested, released and deployed.
