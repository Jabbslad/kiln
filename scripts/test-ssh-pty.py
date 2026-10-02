#!/usr/bin/env python3
"""Exercise a real interactive OpenSSH terminal using a disposable SSH config."""

import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time

pid, master = pty.fork()
if pid == 0:
    os.execvp("ssh", ["ssh", "-tt", "-F", sys.argv[1], sys.argv[2]])
output = b""


def size(rows, columns):
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
    os.kill(pid, signal.SIGWINCH)


def expect(value):
    global output
    deadline = time.monotonic() + 15
    while value not in output:
        if time.monotonic() > deadline:
            raise AssertionError(f"missing {value!r}: {output[-3000:]!r}")
        if select.select([master], [], [], 0.1)[0]:
            output += os.read(master, 8192)
    output = b""


try:
    size(31, 97)
    os.write(master, b"printf '__SIZE__'; stty size\n")
    expect(b"__SIZE__31 97")
    size(43, 111)
    os.write(master, b"printf '__RESIZE__'; stty size\n")
    expect(b"__RESIZE__43 111")
    os.write(master, b"echo __SLEEPING__; sleep 30\n")
    expect(b"\r\n__SLEEPING__\r\n")
    os.write(master, b"\x03")
    os.write(master, b"printf '\\n__INTERRUPTED__\\n'\n")
    expect(b"\r\n__INTERRUPTED__\r\n")
    os.write(master, b"exit\n")
    _, status = os.waitpid(pid, 0)
    assert os.waitstatus_to_exitcode(status) == 0
    print("PASS: interactive PTY, terminal resize and Ctrl-C")
finally:
    os.close(master)
