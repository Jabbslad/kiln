#!/usr/bin/env python3
"""Check an already approved central profile against an existing disposable box.

Uses the real CLI, HTTPS gateway, guest exec and SSH. Does not log in, enroll,
create/delete boxes, change server configuration, or print credentials. Run only
after completing real provider consent and host enrollment. This is not a fixture
substitute for those steps or for refresh/revocation/outage acceptance.
"""

import argparse
import json
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="kiln")
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--profile", default="default")
    parser.add_argument("--box", required=True, help="existing disposable running box ID")
    args = parser.parse_args()
    if len(args.box) != 32 or any(c not in "0123456789abcdef" for c in args.box):
        parser.error("box must be a 32-character hexadecimal ID")
    profile = json.loads(args.config.read_text())["profiles"][args.profile]
    if "issuer" not in profile or "credential_file" not in profile or "token_file" in profile:
        parser.error("acceptance requires a central profile, not administrator credentials")
    base = [args.binary, "--config", str(args.config), "--profile", args.profile]

    def run(arguments, expected=0, stdout=None, stderr=None):
        result = subprocess.run(base + arguments, capture_output=True, timeout=120)
        if result.returncode != expected:
            raise RuntimeError(f"{arguments[0]} returned {result.returncode}, expected {expected}")
        if stdout is not None and result.stdout != stdout:
            raise RuntimeError(f"{arguments[0]} stdout differed")
        if stderr is not None:
            # CLI request IDs are local diagnostics, not guest stderr.
            lines = result.stderr.splitlines(keepends=True)
            guest = b"".join(line for line in lines if not line.startswith(b"request "))
            if guest != stderr:
                raise RuntimeError(f"{arguments[0]} stderr differed")
        return result.stdout

    status = json.loads(run(["auth", "status"]))
    if not status.get("device_id") or not status.get("account_id"):
        raise RuntimeError("device identity was not returned")
    templates = json.loads(run(["--json", "templates"]))
    if not templates:
        raise RuntimeError("no templates discovered")
    inventory = json.loads(run(["--json", "list"]))
    box = json.loads(run(["--json", "inspect", args.box]))
    if box["id"] != args.box or box["state"] != "running":
        raise RuntimeError("acceptance box is not running")
    if not any(item["id"] == args.box for item in inventory):
        raise RuntimeError("acceptance box missing from inventory")
    run(
        ["exec", args.box, "--", "python3", "-c",
         "import os; os.write(1,bytes([0,255,128,65])); os.write(2,b'guest-error\\n'); raise SystemExit(23)"],
        expected=23, stdout=bytes([0,255,128,65]), stderr=b"guest-error\n",
    )
    run(
        ["ssh", args.box, "--", "sh", "-c", "'printf auth-ssh-ok; exit 37'"],
        expected=37, stdout=b"auth-ssh-ok",
    )
    print(json.dumps({
        "result": "pass", "checks": ["central-device-status", "templates", "list", "inspect", "binary-exec-exit23", "ssh-exit37"],
        "limitations": ["assumes completed real browser consent and enrollment", "SFTP, interactive PTY, concurrent refresh, revocation and outages require separate acceptance"],
    }, indent=2))


if __name__ == "__main__":
    main()
