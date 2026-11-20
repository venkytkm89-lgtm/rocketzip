"""Draws RocketZip's icon (original design) and saves rocketzip.ico / rocketzip.png.
Run: python installer/make_icon.py   (needs Pillow)"""
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent.parent / "rocketzip" / "assets"
S = 1024  # draw large, then downscale for smooth edges


def draw() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # tile with a vertical gradient
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / S
        gd.line([(0, y), (S, y)], fill=(int(24 + 30 * t), int(86 - 40 * t), int(196 - 60 * t), 255))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([40, 40, S - 40, S - 40], radius=200, fill=255)
    img.paste(grad, (0, 0), mask)
    # white "Z" built from three bars
    w = 120
    top, bot, left, right = 250, S - 250, 260, S - 260
    d.rectangle([left, top, right, top + w], fill="white")
    d.rectangle([left, bot - w, right, bot], fill="white")
    d.polygon([(right - 20, top + w), (right, top + w), (left + 20, bot - w), (left, bot - w)],
              fill="white")
    d.polygon([(right - 150, top + w), (right, top + w), (left + 150, bot - w), (left, bot - w)],
              fill="white")
    # orange clamp squeezing the Z (compression)
    d.rounded_rectangle([150, S // 2 - 55, S - 150, S // 2 + 55], radius=40,
                        fill=(255, 138, 30, 255), outline=(150, 70, 0, 255), width=14)
    for x in (230, S - 230):  # rivets
        d.ellipse([x - 26, S // 2 - 26, x + 26, S // 2 + 26], fill=(150, 70, 0, 255))
    return img


if __name__ == "__main__":
    big = draw()
    big.resize((256, 256), Image.LANCZOS).save(HERE / "rocketzip.png")
    big.resize((256, 256), Image.LANCZOS).save(
        HERE / "rocketzip.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64),
                                      (128, 128), (256, 256)])
    print("wrote", HERE / "rocketzip.ico")
