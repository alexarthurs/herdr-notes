import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time


def drain(master: int, duration: float) -> bytes:
    output = bytearray()
    deadline = time.monotonic() + duration
    while (remaining := deadline - time.monotonic()) > 0:
        if not select.select([master], [], [], remaining)[0]:
            break
        try:
            chunk = os.read(master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                break
            raise
        if not chunk:
            break
        output.extend(chunk)
    return bytes(output)


def check(binary: Path, text: str, keys: bytes, expected: str, autosave: bool = False) -> None:
    with tempfile.TemporaryDirectory(prefix="herdr_notes_keys_") as directory:
        state = Path(directory) / "wkeys.json"
        state.write_text(json.dumps({"text": text, "mode": "edit"}))
        environment = dict(os.environ)
        for name in ("HERDR_PANE_ID", "HERDR_SOCKET_PATH", "HERDR_BIN_PATH"):
            environment.pop(name, None)
        environment.update(
            TERM="xterm-256color",
            HERDR_PLUGIN_STATE_DIR=directory,
            HERDR_WORKSPACE_ID="wkeys",
        )
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
        process = subprocess.Popen(
            [str(binary)], stdin=slave, stdout=slave, stderr=slave, env=environment,
        )
        os.close(slave)
        try:
            output = drain(master, 0.2)
            assert process.poll() is None, output.decode(errors="replace")
            os.write(master, keys)
            drain(master, 2.6 if autosave else 0.1)
            if autosave:
                assert json.loads(state.read_text())["text"] == expected
            os.write(master, b"\x1b")
            drain(master, 0.1)
            os.write(master, b"q")
            drain(master, 0.1)
            assert process.wait(timeout=3) == 0
            actual = json.loads(state.read_text())
            assert actual == {"text": expected, "mode": "preview"}, actual
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=3)
            os.close(master)


def main() -> None:
    parser = argparse.ArgumentParser(description="Check terminal shortcut bytes against the notes executable")
    parser.add_argument("binary", type=Path)
    binary = parser.parse_args().binary.resolve()
    right = b"\x1b[C"
    cases = [
        ("Cmd+Right / Ctrl+E", "one two three", right * 2 + b"\x05!", "one two three!"),
        ("Cmd+Left / Ctrl+A", "one two three", right * 7 + b"\x01!", "!one two three"),
        ("Option+Left / Esc B", "one two three", right * 7 + b"\x1bb!", "one !two three"),
        ("Option+Right / Esc F", "one two three", right * 4 + b"\x1bf!", "one two! three"),
        ("Alt+Left CSI", "one two three", right * 7 + b"\x1b[1;3D!", "one !two three"),
        ("Alt+Right CSI", "one two three", right * 4 + b"\x1b[1;3C!", "one two! three"),
        ("Cmd+Backspace / Ctrl+U", "one two three", right * 7 + b"\x15", " three"),
        ("Option+Backspace", "one two three", right * 7 + b"\x1b\x7f", "one  three"),
        ("Ctrl+W word deletion", "one two three", right * 7 + b"\x17", "one  three"),
        ("Ctrl+K line suffix", "one two three", right * 7 + b"\x0b", "one two"),
        ("Alt+D word suffix", "one two three", right * 4 + b"\x1bd", "one  three"),
        ("Unicode word deletion", "café 中文", right * 7 + b"\x1b\x7f", "café "),
        ("Multiline word deletion", "one\n\n  two\nthree", b"\x1b[B" * 2 + right * 2 + b"\x1b\x7f", "two\nthree"),
    ]
    failures = []
    for name, text, keys, expected in cases:
        try:
            check(binary, text, keys, expected)
        except (AssertionError, subprocess.TimeoutExpired) as error:
            failures.append(name)
            print(f"FAIL {name}: {error}")
        else:
            print(f"PASS {name}")
    check(binary, "one two three", right * 7 + b"\x15", " three", autosave=True)
    print("PASS deletion autosaves while editing")
    assert not failures, f"{len(failures)} shortcut regressions: {failures}"
    print(f"{len(cases) + 1} terminal checks passed")


if __name__ == "__main__":
    main()
