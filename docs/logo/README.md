# Logo

The app logo is **„Check-Kuvert“** (check envelope): an envelope with a check mark that reaches
beyond its edge (mail = completed task). Colour: `--accent` `#2f6fe4`, gradient `#4a8bf5` → `#1d4fb8`.

| File | Contents |
| --- | --- |
| `src-tauri/icons/icon.svg` | **Vector master** of the app icon (1024 × 1024) |
| `docs/logo/1-check-kuvert.svg` | the same file, alongside the other drafts |
| `docs/logo/2-…5-*.svg` | rejected alternatives |
| `docs/logo/vorschau.html` | all drafts at 192 / 96 / 48 / 32 / 16 px, light and dark |

## Regenerating the app icons

After changing `src-tauri/icons/icon.svg`:

1. Render the SVG as a 1024 × 1024 PNG with a transparent background. The shadow is an
   SVG filter, so use a browser renderer, e.g. Chromium. The window is deliberately
   taller than 1024 px because headless Chromium otherwise cuts off the bottom edge; the script
   crops the square from the top left again:

   ```bash
   chromium --headless --disable-gpu --default-background-color=00000000 \
     --hide-scrollbars --window-size=1024,1200 --screenshot=logo-1024.png src-tauri/icons/icon.svg
   ```

2. Generate the sizes and formats from it (requires Pillow, `pip install pillow`):

   ```bash
   python3 docs/logo/make-icons.py logo-1024.png
   ```

   This writes `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png`, `icon.ico`
   (16–256 px) and `icon.icns` to `src-tauri/icons/` – exactly the files listed in
   `tauri.conf.json`.

Alternatively, `cargo tauri icon logo-1024.png` works; it also creates icons for the Windows Store,
which the project does not use.

**Android and iOS** icons come from the same master, rendered in a browser (drop shadow) and
generated with `cargo tauri icon`:
- Android (`src-tauri/gen/android/app/src/main/res/`): launcher icons per density, adaptive icon
  (`mipmap-anydpi-v26/`, gradient background, logo inside the 66 dp safe zone) with a monochrome
  layer for themed icons (`drawable/ic_launcher_monochrome.xml`), and the white status bar icon
  `drawable/ic_stat_mail.xml`.
- iOS (`src-tauri/icons/ios/`): the AppIcon set, without alpha (Apple rejects transparent app
  icons). Copy it into `Assets.xcassets/AppIcon.appiconset` once `tauri ios init` has created
  `gen/apple`.
