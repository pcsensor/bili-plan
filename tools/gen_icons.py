#!/usr/bin/env python3
"""Generate bili-planner icon assets (PNG + ICO) matching the runtime-drawn icon.

Design (same as src/main.rs icon_bytes): System Blue (#0A84FF) circular disc with a
centered white play triangle (vertices 12,10 / 22,15.5 / 12,21 in a 32x32 grid).
Rendered with supersampling for smooth anti-aliased edges.
"""
import math
import os
import shutil
import struct
import zlib

SIZES = [16, 24, 32, 48, 64, 128, 256]
OUT_DIR = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "icons"))
BLUE = (10, 132, 255)  # System Blue #0A84FF


def coverage(px, py, radius, cx, cy, tri):
    """Return (circle_alpha, inside_triangle) for pixel center (px, py)."""
    d = math.hypot(px - cx, py - cy) - radius
    circle_alpha = max(0.0, min(1.0, 0.5 - d))  # linear AA over ~1px
    (ax, ay), (bx, by), (c2x, c2y) = tri
    d1 = (px - bx) * (ay - by) - (ax - bx) * (py - by)
    d2 = (px - c2x) * (by - c2y) - (bx - c2x) * (py - c2y)
    d3 = (px - ax) * (c2y - ay) - (c2x - ax) * (py - ay)
    neg = (d1 < 0.0) or (d2 < 0.0) or (d3 < 0.0)
    pos = (d1 > 0.0) or (d2 > 0.0) or (d3 > 0.0)
    inside = not (neg and pos)
    return circle_alpha, inside


def render(size, ss=4):
    """Render RGBA bytes for a given icon size using 4x4 supersampling."""
    buf = bytearray(size * size * 4)

    def s(v):
        return v * (size * ss) / 32.0

    radius = s(14.0)
    cx, cy = s(15.5), s(15.5)
    tri = ((s(12.0), s(10.0)), (s(22.0), s(15.5)), (s(12.0), s(21.0)))
    for y in range(size):
        for x in range(size):
            acc_r = acc_g = acc_b = acc_a = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    px = (x * ss + sx + 0.5) / ss
                    py = (y * ss + sy + 0.5) / ss
                    ca, inside = coverage(px, py, radius, cx, cy, tri)
                    if ca <= 0.0:
                        continue
                    r, g, b = (255, 255, 255) if inside else BLUE
                    acc_r += r * ca
                    acc_g += g * ca
                    acc_b += b * ca
                    acc_a += 255 * ca
            i = (y * size + x) * 4
            if acc_a > 0:
                buf[i] = round(acc_r / acc_a)
                buf[i + 1] = round(acc_g / acc_a)
                buf[i + 2] = round(acc_b / acc_a)
            buf[i + 3] = round(acc_a / (ss * ss))
    return bytes(buf)


def write_png(path, size, rgba):
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        c += struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        return c

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    raw = b"".join(b"\x00" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))
    idat = zlib.compress(raw, 9)
    png = sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)
    return png


def write_ico(path, pngs):
    """Write a multi-image ICO containing PNG-compressed entries."""
    count = len(pngs)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    data = b""
    for (w, png) in pngs:
        b_w = 0 if w >= 256 else w
        entries += struct.pack("<BBBBHHII", b_w, b_w, 0, 0, 1, 32, len(png), offset)
        data += png
        offset += len(png)
    with open(path, "wb") as f:
        f.write(header + entries + data)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    pngs = []
    for size in SIZES:
        rgba = render(size)
        p = os.path.join(OUT_DIR, f"{size}x{size}.png")
        png = write_png(p, size, rgba)
        pngs.append((size, png))
        print("wrote", p)
    ico = os.path.join(OUT_DIR, "icon.ico")
    write_ico(ico, pngs)
    print("wrote", ico)
    icon_png = os.path.join(OUT_DIR, "icon.png")
    shutil.copy(os.path.join(OUT_DIR, "256x256.png"), icon_png)
    print("wrote", icon_png)


if __name__ == "__main__":
    main()
