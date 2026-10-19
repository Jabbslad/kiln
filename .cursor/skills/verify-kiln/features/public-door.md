# Public door

A laptop installs the client, runs `kiln login`, and then `kiln list`, `kiln create`, `kiln exec`, and `kiln delete` against dark-forge. The identity page stays `https://dark-forge.dev`. The API name is `https://cli.dark-forge.dev:8443`. The door on that name checks an identity JWT before it dials loopback `kiln-api`. A request without that JWT does not open the dial. The administrator token is not accepted on the door.

The approving GitHub account for the real host is login Jabbslad and numeric id 3084. `deploy/enroll-dark-forge.sh` is the operator command. It is shell. It refuses to replace an enrollment it did not create in that run, and it checks the stored URL `https://cli.dark-forge.dev:8443`. The runbook is `docs/runbooks/2026-10-19-public-door.md`. Do not deploy or restart the real server from this skill.

SSH is not served by the public door.

## How the lab proof is shaped

`scripts/test-public-door.sh` builds a local stack. The client runs in a network namespace that cannot dial host loopback. Names `login.public.test`, `cli.public.test`, and `github.public.test` point at a veth address. Identity stays on `127.0.0.1:8091` with a TLS proxy in front. A stand-in answers the GitHub token URLs. The stand-in is refused when the identity origin is `https://dark-forge.dev`.

The client sets `KILN_IDENTITY_CA` only for that lab certificate. The same variable is refused for `dark-forge.dev`. Production laptops do not set it.

The guest path needs working KVM. This proof uses the development profile because it is a lab host, not the isolated dark-forge profile.

## Driving it

```sh
bash scripts/test-public-door.sh
```

A pass prints `public door laptop path passed`, creates a box, runs `/bin/echo public-door`, and deletes the box. Earlier lines show HTTP 401 for a missing credential, the administrator token, and a token minted for a different account. `curl` from the client namespace to `https://127.0.0.1:8443` fails to connect.

## Gotchas

- Do not bind the door on `0.0.0.0`. That bind also takes `kiln-api`'s loopback port.
- Enrollment readiness uses `--local-url https://127.0.0.1:8443`. The public URL is what the laptop stores.
- Release `kiln-identity` has no `--github-*-url` or `--provider-ca-file` flags. The `lab-provider` feature is off by default and is what this lab build enables. `https://dark-forge.dev` still refuses a substitute provider when the feature is on. `kiln-identity.service` is not edited.
- Creating the guest needs `KVM_CREATE_VCPU` to succeed. A host kernel that oopses there cannot boot Firecracker. The login, list, and rejection checks still run before that step.
