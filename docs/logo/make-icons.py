"""Erzeugt die Tauri-Programmsymbole aus einem 1024×1024-PNG des Logos.

Aufruf:  python3 docs/logo/make-icons.py logo-1024.png

Ist das PNG höher als breit (Headless-Chromium-Screenshot), wird das Quadrat oben links genommen.
"""
import sys
from PIL import Image

img = Image.open(sys.argv[1]).convert('RGBA')
src = img.crop((0, 0, img.width, img.width))
out = 'src-tauri/icons/'


def size(s):
    return src.resize((s, s), Image.LANCZOS)


size(32).save(out + '32x32.png')
size(128).save(out + '128x128.png')
size(256).save(out + '128x128@2x.png')
size(512).save(out + 'icon.png')
src.save(out + 'icon.ico', sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (256, 256)])
src.save(out + 'icon.icns')
