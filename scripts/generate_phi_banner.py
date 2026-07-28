#!/usr/bin/env python3
import argparse
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

DOTS = {
    (0, 0): 1,
    (0, 1): 2,
    (0, 2): 4,
    (0, 3): 64,
    (1, 0): 8,
    (1, 1): 16,
    (1, 2): 32,
    (1, 3): 128,
}

BAYER_4 = (
    (0, 8, 2, 10),
    (12, 4, 14, 6),
    (3, 11, 1, 9),
    (15, 7, 13, 5),
)

AGENT_ANSI_SHADOW = (
    " █████╗  ██████╗ ███████╗███╗   ██╗████████╗",
    "██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝",
    "███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   ",
    "██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   ",
    "██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   ",
    "╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝   ",
)


def render_svg(svg: Path) -> Image.Image:
    with tempfile.TemporaryDirectory() as directory:
        png = Path(directory) / "phi-logo.png"
        subprocess.run(
            [
                "convert",
                "-background",
                "white",
                "-alpha",
                "remove",
                "-alpha",
                "off",
                str(svg),
                str(png),
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        return Image.open(png).convert("L")


def content_box(image: Image.Image, threshold: int, padding: int) -> tuple[int, int, int, int]:
    pixels = image.load()
    xs = []
    ys = []
    for y in range(image.height):
        for x in range(image.width):
            if pixels[x, y] < threshold:
                xs.append(x)
                ys.append(y)
    if not xs:
        raise ValueError("logo raster has no visible dark pixels")
    return (
        max(0, min(xs) - padding),
        max(0, min(ys) - padding),
        min(image.width, max(xs) + 1 + padding),
        min(image.height, max(ys) + 1 + padding),
    )


def braille_banner(
    image: Image.Image,
    cols: int,
    crop_threshold: int,
    ink_threshold: int,
    dither: float,
    padding: int,
) -> str:
    cropped = image.crop(content_box(image, crop_threshold, padding))
    pixel_width = cols * 2
    pixel_height = round(pixel_width * cropped.height / cropped.width)
    pixel_height = max(4, round(pixel_height / 4) * 4)
    resized = cropped.resize((pixel_width, pixel_height), Image.Resampling.LANCZOS)

    rows = []
    for cell_y in range(pixel_height // 4):
        row = ""
        for cell_x in range(cols):
            bits = 0
            for dot_y in range(4):
                for dot_x in range(2):
                    x = cell_x * 2 + dot_x
                    y = cell_y * 4 + dot_y
                    local_threshold = ink_threshold + (BAYER_4[y % 4][x % 4] - 7.5) * dither
                    if resized.getpixel((x, y)) < local_threshold:
                        bits |= DOTS[(dot_x, dot_y)]
            row += chr(0x2800 + bits) if bits else " "
        rows.append(row.rstrip())
    return "\n".join(rows)


def compose_banner(
    logo: str, label: tuple[str, ...], gap: int, label_drop: int, indent: int
) -> str:
    logo_lines = logo.splitlines()
    width = max((len(line) for line in logo_lines), default=0)
    label_start = label_drop
    height = max(len(logo_lines), label_start + len(label))
    separator = " " * gap
    prefix = " " * indent
    rows = []
    for index in range(height):
        logo_line = logo_lines[index] if index < len(logo_lines) else ""
        label_index = index - label_start
        label_line = label[label_index] if 0 <= label_index < len(label) else ""
        rows.append(f"{prefix}{logo_line.ljust(width)}{separator}{label_line.rstrip()}")
    return "\n".join(row.rstrip() for row in rows)


def write_banner(path: Path, banner: str) -> None:
    source = path.read_text(encoding="utf-8")
    start = source.index('pub(crate) const PHI_BANNER: &str = r#"\n') + len(
        'pub(crate) const PHI_BANNER: &str = r#"\n'
    )
    end = source.index('\n"#;', start)
    path.write_text(source[:start] + banner + source[end:], encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate Phi's Unicode startup banner.")
    parser.add_argument("--svg", type=Path, default=Path("assets/phi-logo.svg"))
    parser.add_argument("--banner-rs", type=Path, default=Path("phi/src/banner.rs"))
    parser.add_argument("--cols", type=int, default=10)
    parser.add_argument("--crop-threshold", type=int, default=245)
    parser.add_argument("--ink-threshold", type=int, default=188)
    parser.add_argument("--dither", type=float, default=5.0)
    parser.add_argument("--padding", type=int, default=0)
    parser.add_argument("--label-gap", type=int, default=3)
    parser.add_argument("--label-drop", type=int, default=1)
    parser.add_argument("--banner-indent", type=int, default=2)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()

    logo = braille_banner(
        render_svg(args.svg),
        args.cols,
        args.crop_threshold,
        args.ink_threshold,
        args.dither,
        args.padding,
    )
    banner = compose_banner(
        logo,
        AGENT_ANSI_SHADOW,
        args.label_gap,
        args.label_drop,
        args.banner_indent,
    )
    if args.write:
        write_banner(args.banner_rs, banner)
    print(banner)


if __name__ == "__main__":
    main()
