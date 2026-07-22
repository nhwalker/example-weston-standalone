"""Minimal RFB (VNC) client for driving westonite's VNC backend in tests.

Deliberately tiny and dependency-light: pure Python plus the
`cryptography` package (RPM: python3-cryptography) for the Apple
Diffie-Hellman authentication (RFB security type 30), which is one of
the three security types neatvnc offers when TLS is disabled
(RSA-AES-256, RSA-AES, Apple DH -- there is no classic VNC auth, which
is why off-the-shelf scriptable clients fail here; see
docs/e2e-test-plan.md spike S1).

Supports exactly what the e2e suite needs:
  - authenticate as a PAM user (Apple DH)
  - full-framebuffer capture (Raw encoding only)
  - pointer and keyboard event injection
"""

import hashlib
import os
import socket
import struct

APPLE_DH = 30
RAW_ENCODING = 0


class RfbError(AssertionError):
    pass


class VncClient:
    def __init__(self, host, port, username, password, timeout=10.0):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        self._handshake(username, password)
        self._client_init()

    # -- wire helpers ---------------------------------------------------

    def _read(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise RfbError(f"server closed connection ({len(buf)}/{n} bytes)")
            buf += chunk
        return buf

    def _send(self, data):
        self.sock.sendall(data)

    # -- handshake ------------------------------------------------------

    def _handshake(self, username, password):
        version = self._read(12)
        if not version.startswith(b"RFB 003."):
            raise RfbError(f"unexpected protocol version {version!r}")
        self._send(b"RFB 003.008\n")

        ntypes = self._read(1)[0]
        if ntypes == 0:
            reason_len = struct.unpack(">I", self._read(4))[0]
            raise RfbError("handshake refused: "
                           + self._read(reason_len).decode(errors="replace"))
        types = self._read(ntypes)
        if APPLE_DH not in types:
            raise RfbError(f"server offers {list(types)}, need {APPLE_DH} (Apple DH)")
        self._send(bytes([APPLE_DH]))
        self._apple_dh_auth(username, password)

        result = struct.unpack(">I", self._read(4))[0]
        if result != 0:
            reason_len = struct.unpack(">I", self._read(4))[0]
            raise RfbError("authentication failed: "
                           + self._read(reason_len).decode(errors="replace"))

    def _apple_dh_auth(self, username, password):
        from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

        generator = int.from_bytes(self._read(2), "big")
        key_len = int.from_bytes(self._read(2), "big")
        prime = int.from_bytes(self._read(key_len), "big")
        server_pub = int.from_bytes(self._read(key_len), "big")

        priv = int.from_bytes(os.urandom(key_len), "big") % prime
        client_pub = pow(generator, priv, prime)
        shared = pow(server_pub, priv, prime)

        aes_key = hashlib.md5(shared.to_bytes(key_len, "big")).digest()
        # 128-byte credential block: username and password each occupy a
        # 64-byte null-terminated field; unused bytes stay random.
        creds = bytearray(os.urandom(128))
        user_bytes = username.encode()[:63] + b"\0"
        pass_bytes = password.encode()[:63] + b"\0"
        creds[0:len(user_bytes)] = user_bytes
        creds[64:64 + len(pass_bytes)] = pass_bytes

        enc = Cipher(algorithms.AES(aes_key), modes.ECB()).encryptor()
        ciphertext = enc.update(bytes(creds)) + enc.finalize()
        self._send(ciphertext + client_pub.to_bytes(key_len, "big"))

    def _client_init(self):
        self._send(b"\x01")  # shared
        head = self._read(24)
        self.width, self.height = struct.unpack(">HH", head[:4])
        name_len = struct.unpack(">I", head[20:24])[0]
        self.name = self._read(name_len).decode(errors="replace")

        # SetPixelFormat: 32bpp truecolor little-endian BGRX
        pixel_format = struct.pack(">BBBBHHHBBBxxx", 32, 24, 0, 1,
                                   255, 255, 255, 16, 8, 0)
        self._send(struct.pack(">Bxxx", 0) + pixel_format)
        # SetEncodings: Raw only -- every update arrives uncompressed
        self._send(struct.pack(">BxH", 2, 1) + struct.pack(">i", RAW_ENCODING))

    # -- framebuffer ----------------------------------------------------

    def capture(self):
        """Request a full non-incremental update; return (width, height,
        bytes) with 4-byte BGRX pixels, row-major."""
        self._send(struct.pack(">BBHHHH", 3, 0, 0, 0, self.width, self.height))
        fb = bytearray(self.width * self.height * 4)
        got_update = False
        while not got_update:
            msg_type = self._read(1)[0]
            if msg_type == 0:  # FramebufferUpdate
                self._read(1)
                nrects = struct.unpack(">H", self._read(2))[0]
                for _ in range(nrects):
                    x, y, w, h, enc = struct.unpack(">HHHHi", self._read(12))
                    if enc != RAW_ENCODING:
                        raise RfbError(f"unexpected encoding {enc}")
                    data = self._read(w * h * 4)
                    for row in range(h):
                        dst = ((y + row) * self.width + x) * 4
                        src = row * w * 4
                        fb[dst:dst + w * 4] = data[src:src + w * 4]
                got_update = True
            elif msg_type == 1:  # SetColourMapEntries
                head = self._read(5)
                ncolours = struct.unpack(">H", head[3:5])[0]
                self._read(6 * ncolours)
            elif msg_type == 2:  # Bell
                pass
            elif msg_type == 3:  # ServerCutText
                length = struct.unpack(">I", self._read(7)[3:])[0]
                self._read(length)
            else:
                raise RfbError(f"unhandled server message type {msg_type}")
        return self.width, self.height, bytes(fb)

    def pixel(self, fb_bytes, x, y):
        """Return (r, g, b) at x,y from a capture() buffer."""
        off = (y * self.width + x) * 4
        b, g, r = fb_bytes[off], fb_bytes[off + 1], fb_bytes[off + 2]
        return (r, g, b)

    # -- input injection ------------------------------------------------

    def pointer(self, x, y, buttons=0):
        self._send(struct.pack(">BBHH", 5, buttons, x, y))

    def click(self, x, y, button=1):
        mask = 1 << (button - 1)
        self.pointer(x, y, 0)
        self.pointer(x, y, mask)
        self.pointer(x, y, 0)

    def key(self, keysym, down):
        self._send(struct.pack(">BBxxI", 4, 1 if down else 0, keysym))

    def key_tap(self, keysym):
        self.key(keysym, True)
        self.key(keysym, False)

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
