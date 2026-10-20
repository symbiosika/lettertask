# Logo

Das App-Logo ist **„Check-Kuvert“**: ein Briefumschlag mit einem Häkchen, das über den Rand
hinausgreift (Mail = erledigte Aufgabe). Farbe: `--accent` `#2f6fe4`, Verlauf `#4a8bf5` → `#1d4fb8`.

| Datei | Inhalt |
| --- | --- |
| `src-tauri/icons/icon.svg` | **Vektor-Vorlage** des App-Icons (1024 × 1024) |
| `docs/logo/1-check-kuvert.svg` | dieselbe Datei, neben den übrigen Entwürfen |
| `docs/logo/2-…5-*.svg` | verworfene Alternativen |
| `docs/logo/vorschau.html` | alle Entwürfe in 192 / 96 / 48 / 32 / 16 px, hell und dunkel |

## Programmsymbole neu erzeugen

Nach einer Änderung an `src-tauri/icons/icon.svg`:

1. SVG als 1024 × 1024-PNG mit transparentem Hintergrund rendern. Der Schatten ist ein
   SVG-Filter, deshalb einen Browser-Renderer nehmen, z. B. Chromium. Das Fenster ist bewusst
   höher als 1024 px, weil Headless-Chromium sonst den unteren Rand abschneidet; das Skript
   schneidet das Quadrat oben links wieder aus:

   ```bash
   chromium --headless --disable-gpu --default-background-color=00000000 \
     --hide-scrollbars --window-size=1024,1200 --screenshot=logo-1024.png src-tauri/icons/icon.svg
   ```

2. Daraus die Größen und Formate erzeugen (benötigt Pillow, `pip install pillow`):

   ```bash
   python3 docs/logo/make-icons.py logo-1024.png
   ```

   Das schreibt `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png`, `icon.ico`
   (16–256 px) und `icon.icns` nach `src-tauri/icons/` – genau die Dateien aus
   `tauri.conf.json`.

Alternativ geht `cargo tauri icon logo-1024.png`; das legt aber zusätzlich Icons für Android,
iOS und den Windows Store an, die das Projekt nicht verwendet.
