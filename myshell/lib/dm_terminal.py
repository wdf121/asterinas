#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Answer guest terminal queries without filtering the QEMU output stream.

The suite's console log is observed from its current end. Commands from stdin
and terminal replies share one writer to the child's stdin. stdout and stderr
remain inherited so the existing FIFO/tee logging path stays unchanged.
"""

import os
import re
import selectors
import subprocess
import sys


QUERY = re.compile(
    rb"\x1bP\+q([0-9a-fA-F;]+)\x1b\\"
    rb"|\x1b\[18t|\x1b\[6n|\x1b\[(\d*);(\d*)H|\x1bc"
)


class TerminalQueries:
    """Handle the name and geometry probes used by systemd and getty.

    This is a limited query responder, not a screen renderer. Cursor moves used
    by geometry probes are clamped to a fixed 24x80 terminal. Unknown XTGETTCAP
    capabilities receive a negative response rather than invented capabilities.
    """

    def __init__(self):
        self.pending = b""
        self.row = self.col = 1

    def feed(self, data):
        self.pending += data
        replies = []
        end = 0
        for match in QUERY.finditer(self.pending):
            token = match.group()
            if match[1] is not None:
                for name in match[1].split(b";"):
                    if name.lower() == b"6e616d65":
                        replies.append(b"\x1bP1+r" + name + b"=787465726d\x1b\\")
                    else:
                        replies.append(b"\x1bP0+r" + name + b"\x1b\\")
            elif token == b"\x1b[18t":
                replies.append(b"\x1b[8;24;80t")
            elif token == b"\x1b[6n":
                replies.append(f"\x1b[{self.row};{self.col}R".encode())
            elif token == b"\x1bc":
                self.row = self.col = 1
            else:
                self.row = min(24, max(1, int(match[2] or b"1")))
                self.col = min(80, max(1, int(match[3] or b"1")))
            end = match.end()
        self.pending = self.pending[end:]
        # Retain split escape sequences, but never retain the whole console log.
        if len(self.pending) > 4096:
            self.pending = self.pending[-256:]
        return b"".join(replies)


def main():
    log_path, *command = sys.argv[1:]
    with open(log_path, "rb") as log:
        log.seek(0, os.SEEK_END)
        child = subprocess.Popen(command, stdin=subprocess.PIPE, bufsize=0)
        terminal = TerminalQueries()
        selector = selectors.DefaultSelector()
        selector.register(sys.stdin.fileno(), selectors.EVENT_READ)

        def send(data):
            view = memoryview(data)
            while view:
                written = os.write(child.stdin.fileno(), view)
                view = view[written:]

        try:
            while child.poll() is None:
                for key, _ in selector.select(0.01):
                    data = os.read(key.fd, 65536)
                    if data:
                        send(data)
                    else:
                        selector.unregister(key.fd)
                        # Keep the pipe open for shutdown terminal queries.
                replies = terminal.feed(log.read(65536))
                if replies:
                    send(replies)
        except BrokenPipeError:
            pass
        finally:
            selector.close()
            child.stdin.close()
        return child.wait()


if __name__ == "__main__":
    sys.exit(main())
