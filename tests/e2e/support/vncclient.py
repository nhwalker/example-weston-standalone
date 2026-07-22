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
DESKTOP_SIZE = -223
EXTENDED_DESKTOP_SIZE = -308


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
        # SetEncodings: Raw pixels (uncompressed) plus the desktop-size
        # pseudo-encodings so the server tells us about output resizes
        encodings = [RAW_ENCODING, DESKTOP_SIZE, EXTENDED_DESKTOP_SIZE]
        self._send(struct.pack(">BxH", 2, len(encodings))
                   + b"".join(struct.pack(">i", e) for e in encodings))
        # screen layout, learned from ExtendedDesktopSize rects
        self.screens = [(0, 0, 0, self.width, self.height, 0)]

    # -- framebuffer ----------------------------------------------------

    def capture(self):
        """Request a full non-incremental update; return (width, height,
        bytes) with 4-byte BGRX pixels, row-major. Desktop-size changes
        arriving in between are absorbed (self.width/height follow)."""
        while True:
            self._send(struct.pack(">BBHHHH", 3, 0, 0, 0,
                                   self.width, self.height))
            fb = bytearray(self.width * self.height * 4)
            got_pixels = False
            resized = False
            while not got_pixels and not resized:
                msg_type = self._read(1)[0]
                if msg_type == 0:  # FramebufferUpdate
                    self._read(1)
                    nrects = struct.unpack(">H", self._read(2))[0]
                    for _ in range(nrects):
                        x, y, w, h, enc = struct.unpack(">HHHHi",
                                                        self._read(12))
                        if enc == RAW_ENCODING:
                            data = self._read(w * h * 4)
                            for row in range(h):
                                dst = ((y + row) * self.width + x) * 4
                                src = row * w * 4
                                fb[dst:dst + w * 4] = data[src:src + w * 4]
                            got_pixels = True
                        elif enc == DESKTOP_SIZE:
                            if (w, h) != (self.width, self.height):
                                self.width, self.height = w, h
                                resized = True
                        elif enc == EXTENDED_DESKTOP_SIZE:
                            # servers repeat the current layout in normal
                            # updates; only a geometry *change* is a resize
                            nscreens = self._read(4)[0]
                            self.screens = [
                                struct.unpack(">IHHHHI", self._read(16))
                                for _ in range(nscreens)]
                            if (w, h) != (self.width, self.height):
                                self.width, self.height = w, h
                                resized = True
                        else:
                            raise RfbError(f"unexpected encoding {enc}")
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
            if got_pixels and not resized:
                return self.width, self.height, bytes(fb)
            # a resize invalidates this update's geometry: re-request

    def set_desktop_size(self, width, height):
        """Ask the server to resize the output (SetDesktopSize). The
        result shows up asynchronously; poll capture() for the change."""
        screen_id, _, _, _, _, flags = self.screens[0]
        msg = struct.pack(">BxHHBx", 251, width, height, 1)
        msg += struct.pack(">IHHHHI", screen_id, 0, 0, width, height, flags)
        self._send(msg)

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

    def drag(self, x0, y0, x1, y1, button=1, steps=8):
        """Press at (x0, y0), move in steps, release at (x1, y1)."""
        mask = 1 << (button - 1)
        self.pointer(x0, y0, 0)
        self.pointer(x0, y0, mask)
        for i in range(1, steps + 1):
            self.pointer(x0 + (x1 - x0) * i // steps,
                         y0 + (y1 - y0) * i // steps, mask)
        self.pointer(x1, y1, 0)

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
