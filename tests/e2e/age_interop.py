#!/usr/bin/env python3
"""Opens a backup bundle's parts with the real `age` command-line tool (ADR 0013): the documented way to read a backup.

    python3 tests/e2e/age_interop.py DIR PASSPHRASE

DIR holds `backup-key.age` and the encrypted exports (`*.age`) taken out of a bundle. The protected private key is opened with the
passphrase (age asks for it on the terminal, so it runs in a pseudo-terminal), then each export is opened with that key, and the
names of the files in each are printed, one export per line: `admin.age: admin.db admin-secrets/admin.key ...`. Exits 2 if `age` is
not installed, so a test can skip, and 1 if anything does not open.
"""
import os
import pty
import shutil
import subprocess
import sys
import tarfile


def main(argv):
    if shutil.which("age") is None:
        print("age is not installed", file=sys.stderr)
        return 2
    directory, passphrase = argv[1], argv[2]
    key = os.path.join(directory, "key.txt")
    pid, fd = pty.fork()
    if pid == 0:
        os.execvp("age", ["age", "-d", "-o", key, os.path.join(directory, "backup-key.age")])
    seen = b""
    sent = False
    while True:
        try:
            data = os.read(fd, 1024)
        except OSError:
            break
        if not data:
            break
        seen += data
        if not sent and b"passphrase" in seen.lower():
            os.write(fd, passphrase.encode() + b"\n")
            sent = True
    _, status = os.waitpid(pid, 0)
    if status != 0 or not os.path.exists(key):
        print("age could not open backup-key.age with the passphrase", file=sys.stderr)
        return 1
    for name in sorted(n for n in os.listdir(directory) if n.endswith(".age") and n != "backup-key.age"):
        out = os.path.join(directory, name + ".tar")
        done = subprocess.run(["age", "-d", "-i", key, "-o", out, os.path.join(directory, name)], capture_output=True)
        if done.returncode != 0:
            print("age could not open " + name, file=sys.stderr)
            return 1
        with tarfile.open(out) as t:
            print(name + ": " + " ".join(m.name for m in t.getmembers()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
