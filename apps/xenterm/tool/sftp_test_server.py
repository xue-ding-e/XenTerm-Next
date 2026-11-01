"""Minimal SSH/SFTP server used to verify XenTerm's real SFTP browser.

Manual acceptance harness (not part of the production build):

    python -m venv .venv && .venv/bin/pip install paramiko
    .venv/bin/python apps/xenterm/tool/sftp_test_server.py

It listens on 127.0.0.1:2222, accepts password demo/demo and any public key,
and serves a test root. Configure with environment variables:
    XENTERM_SFTP_PORT      default 2222
    XENTERM_SFTP_ROOT      default <temp>/xenterm-sftp-root
    XENTERM_SFTP_HOSTKEY   default <temp>/xenterm-sftp-host-key
The generated host key line is printed and written to <hostkey>.pub so it can be
seeded into the app profile's known_hosts for a prompt-free connect.
"""

import os
import socket
import threading
import time

import paramiko

HOST = "127.0.0.1"
PORT = int(os.environ.get("XENTERM_SFTP_PORT", "2222"))
ROOT = os.environ.get(
    "XENTERM_SFTP_ROOT", os.path.join(os.environ.get("TEMP", "/tmp"), "xenterm-sftp-root")
)
HOST_KEY_PATH = os.environ.get(
    "XENTERM_SFTP_HOSTKEY",
    os.path.join(os.environ.get("TEMP", "/tmp"), "xenterm-sftp-host-key"),
)


def ensure_host_key():
    if os.path.exists(HOST_KEY_PATH):
        return paramiko.RSAKey(filename=HOST_KEY_PATH)
    key = paramiko.RSAKey.generate(2048)
    key.write_private_key_file(HOST_KEY_PATH)
    return key


class SftpInterface(paramiko.SFTPServerInterface):
    def _real(self, path):
        path = (path or ".").replace("\\", "/")
        if path in ("", ".", "~", "/"):
            return ROOT
        if path.startswith("~/"):
            path = path[2:]
        path = path.lstrip("/")
        resolved = os.path.normpath(os.path.join(ROOT, path))
        if not resolved.startswith(os.path.normpath(ROOT)):
            raise OSError("path escapes the test root")
        return resolved

    def canonicalize(self, path):
        if path in ("", "."):
            return "."
        return "/" + path.replace("\\", "/").strip("/")

    def list_folder(self, path):
        real = self._real(path)
        out = []
        for name in sorted(os.listdir(real)):
            full = os.path.join(real, name)
            try:
                attr = paramiko.SFTPAttributes.from_stat(os.stat(full))
            except OSError:
                continue
            attr.filename = name
            out.append(attr)
        return out

    def stat(self, path):
        try:
            attr = paramiko.SFTPAttributes.from_stat(os.stat(self._real(path)))
            attr.filename = os.path.basename(self._real(path)) or "/"
            return attr
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def lstat(self, path):
        return self.stat(path)

    def open(self, path, flags, attr):
        real = self._real(path)
        writing = bool(flags & (os.O_WRONLY | os.O_RDWR))
        if writing:
            mode = "wb+" if (flags & os.O_TRUNC or not os.path.exists(real)) else "r+b"
        else:
            mode = "rb"
        try:
            fileobj = open(real, mode)
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)
        return _Handle(fileobj, flags)

    def mkdir(self, path, attr):
        try:
            os.mkdir(self._real(path), 0o755)
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def rmdir(self, path):
        try:
            os.rmdir(self._real(path))
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def remove(self, path):
        try:
            os.remove(self._real(path))
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def rename(self, oldpath, newpath):
        try:
            os.replace(self._real(oldpath), self._real(newpath))
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)


class _Handle(paramiko.SFTPHandle):
    """SFTPHandle must be subclassed: the server only sends a HANDLE response
    for SFTPHandle instances; any other object is treated as an error code."""

    def __init__(self, fileobj, flags):
        super().__init__(flags)
        self._file = fileobj
        self.readfile = fileobj
        self.writefile = fileobj

    def stat(self):
        return paramiko.SFTPAttributes.from_stat(os.fstat(self._file.fileno()))


def monitor_block(tick):
    """One `/proc`-style sample in the format the core's parser expects."""
    user = 1200 + tick * 220
    system = 400 + tick * 60
    idle = 9000 + tick * 260
    rx = 100_000 + tick * 40_000
    tx = 50_000 + tick * 18_000
    return (
        f"cpu  {user} 0 {system} {idle} 20 0 0 0\n"
        "MemTotal:       2048000 kB\n"
        "MemAvailable:   1024000 kB\n"
        "Buffers:          20000 kB\n"
        "Cached:          300000 kB\n"
        "SwapTotal:       512000 kB\n"
        "SwapFree:        256000 kB\n"
        "Inter-|   Receive                                                |  Transmit\n"
        " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n"
        f"  eth0: {rx} 10 0 0 0 0 0 0 {tx} 10 0 0 0 0 0 0\n"
        "__DF__\n"
        "Filesystem     1024-blocks    Used Available Capacity Mounted on\n"
        "/dev/sda1        1000000  400000    600000      40% /\n"
        "__GPU__\n"
        "__GPU_NOSMI__\n"
        "__OS__\n"
        "ID=ubuntu\n"
        "Linux\n"
        "__MSTICK__\n"
    )


def monitor_stream(channel):
    tick = 0
    try:
        while not channel.closed:
            tick += 1
            channel.sendall(monitor_block(tick).encode())
            time.sleep(2)
    except Exception:
        pass


def process_stream(channel):
    try:
        while not channel.closed:
            channel.sendall(
                (
                    "__ME__\n"
                    "demo\n"
                    "__PS__\n"
                    "  PID USER      %CPU %MEM COMMAND\n"
                    "  101 demo       3.5  1.2 python\n"
                    "  202 demo       1.0  0.4 sshd\n"
                    "__PSTICK__\n"
                ).encode()
            )
            time.sleep(2)
    except Exception:
        pass


def echo_shell(channel):
    """A minimal interactive console so the real terminal/trigger paths can be
    exercised end to end: input is echoed back line by line, a few commands
    get real answers, and a "$ " prompt is printed after each reply.

    Line endings follow a real PTY: the Enter key arrives as CR and is echoed
    as CRLF, and every reply line ends CRLF. Stripping CR (as earlier
    revisions did) leaves the terminal column where it was, so the next
    prompt rendered mid-line — the app was drawing exactly what it got.
    """
    prompt = "demo@testhost:~$ "
    try:
        channel.sendall(prompt.encode())
        line = b""
        while not channel.closed:
            data = channel.recv(4096)
            if not data:
                break
            # Console echo: a bare CR becomes CRLF like a real line edit.
            channel.sendall(data.replace(b"\r", b"\r\n"))
            line += data.replace(b"\r", b"").replace(b"\n", b"")
            if b"\n" not in data and b"\r" not in data:
                continue
            command = line.decode("utf-8", "replace").strip()
            line = b""
            reply = ""
            if command == "whoami":
                reply = "demo\r\n"
            elif command == "hostname":
                reply = "testhost\r\n"
            elif command == "uname -s":
                reply = "Linux\r\n"
            elif command.startswith("echo "):
                reply = command[5:] + "\r\n"
            elif command in ("exit", "logout"):
                channel.send_exit_status(0)
                channel.close()
                return
            elif command:
                reply = f"sh: {command.split()[0]}: not found\r\n"
            channel.sendall(reply.encode())
            channel.sendall(prompt.encode())
    except Exception:
        pass


class Server(paramiko.ServerInterface):
    def __init__(self):
        self.direct_requests = {}

    def check_auth_password(self, username, password):
        ok = username == "demo" and password == "demo"
        print("auth password", username, repr(password), ok, flush=True)
        return paramiko.AUTH_SUCCESSFUL if ok else paramiko.AUTH_FAILED

    def check_auth_publickey(self, username, key):
        print("auth publickey", username, flush=True)
        return paramiko.AUTH_SUCCESSFUL

    def get_allowed_auths(self, username):
        return "publickey,password"

    def check_channel_request(self, kind, chanid):
        print("channel request", kind, flush=True)
        return paramiko.OPEN_SUCCEEDED

    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        """Accept local (-L) forwarding and remember where to connect."""
        print("direct-tcpip", origin, "->", destination, flush=True)
        self.direct_requests[chanid] = destination
        return paramiko.OPEN_SUCCEEDED

    def check_channel_shell_request(self, channel):
        channel.send("XenTerm SFTP test server\r\n")
        threading.Thread(target=echo_shell, args=(channel,), daemon=True).start()
        return True

    def check_channel_pty_request(self, channel, *args):
        return True

    def check_channel_exec_request(self, channel, command):
        """Serve the core's auxiliary monitor channels.

        The resource monitor opens a separate exec channel running a small
        `/proc` loop, and the process monitor does the same with `ps`. Both are
        answered here with fixture samples so the Flutter resource panel can be
        accepted end to end.
        """
        text = (
            command.decode("utf-8", "replace")
            if isinstance(command, (bytes, bytearray))
            else str(command)
        )
        print("exec request", text[:48], flush=True)
        if "__MSTICK__" in text and "while :" in text and "__DF__" in text:
            threading.Thread(target=monitor_stream, args=(channel,), daemon=True).start()
            return True
        if "__PSTICK__" in text:
            threading.Thread(target=process_stream, args=(channel,), daemon=True).start()
            return True
        if "__SYS__" in text:
            channel.sendall(
                (
                    "MemTotal:       2048000 kB\n"
                    "MemAvailable:   1024000 kB\n"
                    "__SYS__\n"
                    "OS=XenTerm test host\n"
                    "KERNEL=Linux\n"
                    "__MSTICK__\n"
                ).encode()
            )
            channel.send_exit_status(0)
            channel.close()
            return True
        channel.send("XenTerm SFTP test server\r\n")
        channel.send_exit_status(0)
        channel.close()
        return True

    def check_channel_subsystem_request(self, channel, name):
        print("subsystem request", name, flush=True)
        if name != "sftp":
            return False
        transport = channel.get_transport()
        handler = paramiko.SFTPServer(
            channel, "sftp", self, sftp_si=SftpInterface
        )
        handler.start()
        return True


def pump_forward(channel, destination):
    """Bridge one forwarded channel to a plain TCP socket."""
    try:
        target = socket.create_connection(destination, timeout=5)
    except OSError as error:
        print("forward connect failed", destination, error, flush=True)
        channel.close()
        return
    print("forward connected", destination, flush=True)

    def channel_to_target():
        try:
            while True:
                data = channel.recv(4096)
                if not data:
                    break
                target.sendall(data)
        except Exception:
            pass
        try:
            target.shutdown(socket.SHUT_WR)
        except OSError:
            pass

    threading.Thread(target=channel_to_target, daemon=True).start()
    try:
        while True:
            data = target.recv(4096)
            if not data:
                break
            channel.sendall(data)
    except Exception:
        pass
    try:
        target.close()
    except OSError:
        pass
    channel.close()


def serve_client(client, addr, host_key):
    print("client connected", addr, flush=True)
    transport = paramiko.Transport(client)
    transport.add_server_key(host_key)
    server = Server()
    try:
        transport.start_server(server=server)
        print("server started active=", transport.is_active(), flush=True)
    except Exception as error:
        import traceback

        traceback.print_exc()
        return
    channels = []
    while transport.is_active():
        # Keep every accepted channel referenced: dropping it would garbage
        # collect the Channel and close it before the subsystem request arrives.
        channel = transport.accept(1)
        if channel is not None:
            channels.append(channel)
            destination = server.direct_requests.pop(channel.get_id(), None)
            if destination is not None:
                threading.Thread(
                    target=pump_forward,
                    args=(channel, destination),
                    daemon=True,
                ).start()
        else:
            threading.Event().wait(0.1)


def echo_server(port):
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", port))
    listener.listen(8)
    print(f"echo listening on 127.0.0.1:{port}", flush=True)
    while True:
        conn, _ = listener.accept()
        threading.Thread(target=echo_one, args=(conn,), daemon=True).start()


def echo_one(conn):
    try:
        while True:
            data = conn.recv(4096)
            if not data:
                break
            conn.sendall(data)
    except OSError:
        pass
    finally:
        conn.close()


def main():
    os.makedirs(ROOT, exist_ok=True)
    with open(os.path.join(ROOT, "readme.txt"), "w", encoding="utf-8") as f:
        f.write("XenTerm SFTP test root\n")
    os.makedirs(os.path.join(ROOT, "docs"), exist_ok=True)
    with open(os.path.join(ROOT, "docs", "guide.md"), "w", encoding="utf-8") as f:
        f.write("# Guide\n")
    with open(os.path.join(ROOT, "notes.md"), "w", encoding="utf-8") as f:
        f.write("notes\n")

    host_key = ensure_host_key()
    line = f"{host_key.get_name()} {host_key.get_base64()}"
    pub_path = HOST_KEY_PATH + ".pub"
    with open(pub_path, "w", encoding="ascii") as f:
        f.write(line + "\n")
    print("hostkey:", line)
    print("root:", ROOT)

    echo_port = int(os.environ.get("XENTERM_SFTP_ECHO_PORT", "18081"))
    threading.Thread(target=echo_server, args=(echo_port,), daemon=True).start()

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((HOST, PORT))
    sock.listen(16)
    print(f"listening on {HOST}:{PORT}")
    while True:
        client, addr = sock.accept()
        threading.Thread(
            target=serve_client, args=(client, addr, host_key), daemon=True
        ).start()


if __name__ == "__main__":
    main()
