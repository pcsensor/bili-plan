#!/usr/bin/env python3
"""Generate Bilibili (哔哩哔哩小电视) icon assets (PNG + ICO + ICNS).

Design: Signature Bilibili Pink (#FB7299) rounded container with the classic white
TV chassis, cute antennas, expressive face ( - ‿ - ), and pink blush cheeks.
Rendered at 2048x2048 and downscaled with Lanczos for anti-aliased, crisp icons.
"""
import os
import shutil
import struct
import subprocess
import tempfile
import zlib
from PIL import Image, ImageDraw

SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
OUT_DIR = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "icons"))


def render_master(size=2048):
    """Render the master Bilibili TV icon at high resolution."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    s = size / 1024.0

    # 1. Base Squircle / Rounded Rectangle
    bg_pad = 72 * s
    bg_r = 210 * s
    bg_box = [bg_pad, bg_pad, size - bg_pad, size - bg_pad]

    pink_img = Image.new("RGBA", (size, size), (251, 114, 153, 255))
    mask = Image.new("L", (size, size), 0)
    mask_draw = ImageDraw.Draw(mask)
    mask_draw.rounded_rectangle(bg_box, radius=bg_r, fill=255)
    img.paste(pink_img, (0, 0), mask)

    # 2. Antennas (White, with rounded ends)
    # Left antenna
    draw.line([370 * s, 360 * s, 260 * s, 215 * s], fill=(255, 255, 255, 255), width=int(32 * s))
    draw.ellipse([238 * s, 193 * s, 282 * s, 237 * s], fill=(255, 255, 255, 255))

    # Right antenna
    draw.line([654 * s, 360 * s, 764 * s, 215 * s], fill=(255, 255, 255, 255), width=int(32 * s))
    draw.ellipse([742 * s, 193 * s, 786 * s, 237 * s], fill=(255, 255, 255, 255))

    # 3. TV Body (White Rounded Rectangle)
    tv_box = [215 * s, 320 * s, 809 * s, 740 * s]
    draw.rounded_rectangle(tv_box, radius=96 * s, fill=(255, 255, 255, 255))

    # 4. TV Feet (Rounded capsules)
    draw.line([330 * s, 730 * s, 290 * s, 805 * s], fill=(255, 255, 255, 255), width=int(30 * s))
    draw.ellipse([275 * s, 790 * s, 305 * s, 820 * s], fill=(255, 255, 255, 255))
    draw.line([694 * s, 730 * s, 734 * s, 805 * s], fill=(255, 255, 255, 255), width=int(30 * s))
    draw.ellipse([719 * s, 790 * s, 749 * s, 820 * s], fill=(255, 255, 255, 255))

    # 5. Face details (Iconic Bilibili TV face #232528)
    face_color = (35, 37, 40, 255)

    # Eyes: ( -   - )
    draw.rounded_rectangle([325 * s, 485 * s, 415 * s, 515 * s], radius=15 * s, fill=face_color)
    draw.rounded_rectangle([609 * s, 485 * s, 699 * s, 515 * s], radius=15 * s, fill=face_color)

    # Mouth: Cute smile arc ( ‿ )
    mouth_box = [467 * s, 535 * s, 557 * s, 605 * s]
    draw.arc(mouth_box, start=15, end=165, fill=face_color, width=int(16 * s))

    # 6. Blush: (#FB7299 under eyes)
    blush_color = (251, 114, 153, 210)
    draw.ellipse([285 * s, 530 * s, 335 * s, 558 * s], fill=blush_color)
    draw.ellipse([689 * s, 530 * s, 739 * s, 558 * s], fill=blush_color)

    return img


def write_ico(path, png_files):
    """Write a multi-image ICO containing PNG-compressed entries."""
    entries_data = []
    for p in png_files:
        with open(p, "rb") as f:
            png_bytes = f.read()
        im = Image.open(p)
        w, h = im.size
        entries_data.append((w, h, png_bytes))

    count = len(entries_data)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    data = b""
    for (w, h, png) in entries_data:
        b_w = 0 if w >= 256 else w
        b_h = 0 if h >= 256 else h
        entries += struct.pack("<BBBBHHII", b_w, b_h, 0, 0, 1, 32, len(png), offset)
        data += png
        offset += len(png)
    with open(path, "wb") as f:
        f.write(header + entries + data)


def generate_icns(master, out_icns_path):
    """Generate macOS .icns using iconutil if available."""
    if shutil.which("iconutil") is None:
        return

    iconset_dir = tempfile.mkdtemp(suffix=".iconset")
    try:
        sizes_map = {
            "icon_16x16.png": 16,
            "icon_16x16@2x.png": 32,
            "icon_32x32.png": 32,
            "icon_32x32@2x.png": 64,
            "icon_128x128.png": 128,
            "icon_128x128@2x.png": 256,
            "icon_256x256.png": 256,
            "icon_256x256@2x.png": 512,
            "icon_512x512.png": 512,
            "icon_512x512@2x.png": 1024,
        }
        for filename, sz in sizes_map.items():
            resized = master.resize((sz, sz), Image.Resampling.LANCZOS)
            resized.save(os.path.join(iconset_dir, filename), format="PNG")
        subprocess.run(["iconutil", "-c", "icns", iconset_dir, "-o", out_icns_path], check=True)
        print("wrote", out_icns_path)
    finally:
        shutil.rmtree(iconset_dir)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    master = render_master(2048)

    ico_pngs = []
    for size in SIZES:
        p = os.path.join(OUT_DIR, f"{size}x{size}.png")
        resized = master.resize((size, size), Image.Resampling.LANCZOS)
        resized.save(p, format="PNG")
        print("wrote", p)
        if size in [16, 24, 32, 48, 64, 128, 256]:
            ico_pngs.append(p)

    ico_path = os.path.join(OUT_DIR, "icon.ico")
    write_ico(ico_path, ico_pngs)
    print("wrote", ico_path)

    icon_png = os.path.join(OUT_DIR, "icon.png")
    shutil.copy(os.path.join(OUT_DIR, "512x512.png"), icon_png)
    print("wrote", icon_png)

    icns_path = os.path.join(OUT_DIR, "icon.icns")
    generate_icns(master, icns_path)


if __name__ == "__main__":
    main()
