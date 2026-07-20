from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
PNG_PATH = ASSETS / "app-icon.png"
ICO_PATH = ASSETS / "app.ico"
CANVAS = 1024
SCALE = 4


def load_font(size: int) -> ImageFont.FreeTypeFont:
    for path in [
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ]:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def lerp(a: int, b: int, t: float) -> int:
    return round(a + (b - a) * t)


def make_gradient(size: int) -> Image.Image:
    left = (5, 158, 244)
    mid = (48, 96, 238)
    right = (228, 90, 236)
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    pixels = image.load()
    for y in range(size):
        for x in range(size):
            tx = x / max(1, size - 1)
            ty = y / max(1, size - 1)
            t = min(1.0, max(0.0, tx * 0.82 + ty * 0.18))
            if t < 0.55:
                local = t / 0.55
                color = tuple(lerp(left[i], mid[i], local) for i in range(3))
            else:
                local = (t - 0.55) / 0.45
                color = tuple(lerp(mid[i], right[i], local) for i in range(3))
            pixels[x, y] = (*color, 255)
    return image


def circle_mask(size: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    inset = round(size * 0.055)
    draw.ellipse((inset, inset, size - inset, size - inset), fill=255)
    return mask.filter(ImageFilter.GaussianBlur(size * 0.0012))


def draw_long_shadow(size: int, text_mask: Image.Image) -> Image.Image:
    shadow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    alpha = text_mask.filter(ImageFilter.GaussianBlur(size * 0.0025))
    step = max(1, size // 96)
    for i in range(1, 32):
        shifted = Image.new("L", (size, size), 0)
        shifted.paste(alpha, (i * step, i * step))
        opacity = max(0, 70 - i * 2)
        layer = Image.new("RGBA", (size, size), (22, 38, 150, opacity))
        shadow.alpha_composite(Image.composite(layer, Image.new("RGBA", (size, size)), shifted))
    return shadow


def text_mask(size: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    font = load_font(round(size * 0.66))
    text = "a"
    bbox = draw.textbbox((0, 0), text, font=font)
    width = bbox[2] - bbox[0]
    height = bbox[3] - bbox[1]
    x = round(size * 0.24 - bbox[0])
    y = round((size - height) / 2 - size * 0.02 - bbox[1])
    draw.text((x, y), text, fill=255, font=font)
    stem_left = round(size * 0.64)
    stem_top = round(size * 0.42)
    stem_right = round(size * 0.735)
    stem_bottom = round(size * 0.78)
    radius = round((stem_right - stem_left) * 0.5)
    draw.rounded_rectangle(
        (stem_left, stem_top, stem_right, stem_bottom),
        radius=radius,
        fill=255,
    )
    dot_radius = round(size * 0.071)
    dot_center = (round(size * 0.687), round(size * 0.31))
    draw.ellipse(
        (
            dot_center[0] - dot_radius,
            dot_center[1] - dot_radius,
            dot_center[0] + dot_radius,
            dot_center[1] + dot_radius,
        ),
        fill=255,
    )
    return mask


def render_icon(size: int) -> Image.Image:
    work = size * SCALE
    base = make_gradient(work)
    circle = circle_mask(work)
    image = Image.new("RGBA", (work, work), (0, 0, 0, 0))
    image.alpha_composite(Image.composite(base, Image.new("RGBA", (work, work)), circle))

    mask = text_mask(work)
    shadow = draw_long_shadow(work, mask)
    shadow.putalpha(Image.composite(shadow.getchannel("A"), Image.new("L", (work, work), 0), circle))
    image.alpha_composite(shadow)

    glyph = Image.new("RGBA", (work, work), (255, 255, 255, 255))
    image.alpha_composite(Image.composite(glyph, Image.new("RGBA", (work, work), (0, 0, 0, 0)), mask))
    image.putalpha(circle)
    return image.resize((size, size), Image.Resampling.LANCZOS)


def assert_round_transparent(image: Image.Image) -> None:
    corners = [
        image.getpixel((0, 0))[3],
        image.getpixel((image.width - 1, 0))[3],
        image.getpixel((0, image.height - 1))[3],
        image.getpixel((image.width - 1, image.height - 1))[3],
    ]
    if any(value != 0 for value in corners):
        raise RuntimeError(f"icon corners are not transparent: {corners}")
    center_alpha = image.getpixel((image.width // 2, image.height // 2))[3]
    if center_alpha < 240:
        raise RuntimeError(f"icon center is unexpectedly transparent: {center_alpha}")
    radius = image.width * 0.42
    for angle in range(0, 360, 30):
        x = round(image.width / 2 + math.cos(math.radians(angle)) * radius)
        y = round(image.height / 2 + math.sin(math.radians(angle)) * radius)
        if image.getpixel((x, y))[3] < 180:
            raise RuntimeError("icon circle edge alpha check failed")


def main() -> None:
    ASSETS.mkdir(parents=True, exist_ok=True)
    png = render_icon(CANVAS)
    assert_round_transparent(png)
    png.save(PNG_PATH)
    sizes = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)]
    png.save(ICO_PATH, sizes=sizes)
    print(f"wrote {PNG_PATH}")
    print(f"wrote {ICO_PATH}")


if __name__ == "__main__":
    main()
