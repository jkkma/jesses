# jesses icon assets

The app icon depicts two rust-orange leather jesses with curved straps and cuff loops on a warm parchment background.

## Master artwork

[`jesses-icon-master.png`](jesses-icon-master.png) is the original selected artwork, preserved without visual changes: 1254 x 1254 pixels, RGB PNG.

SHA-256:

```text
356d27bfc2d29f5902cd5202007bd4ad9a831772aba6419281f209db5dabe527
```

## Desktop assets

Generated assets live in [`src-tauri/icons`](../../src-tauri/icons):

- `icon.ico`: Windows icon layers.
- `icon.icns`: macOS icon layers.
- `32x32.png`, `64x64.png`, `128x128.png`, `128x128@2x.png`, and `icon.png`: desktop PNG sizes.
- `Square*Logo.png` and `StoreLogo.png`: additional Windows packaging sizes emitted by the generator.

Regenerate from the repository root using the pinned Tauri CLI:

```sh
npx --yes @tauri-apps/cli@2.11.4 icon assets/branding/jesses-icon-master.png --output src-tauri/icons
```

The generator also emits Android and iOS directories; those outputs are ignored because this application targets desktop platforms. The CLI handles resizing and container formats. Keep the master artwork intact.

When adding the application scaffold, set Tauri's `bundle.icon` to these paths relative to `src-tauri/tauri.conf.json`:

```json
[
  "icons/32x32.png",
  "icons/128x128.png",
  "icons/128x128@2x.png",
  "icons/icon.icns",
  "icons/icon.ico"
]
```

See the [Tauri app icon documentation](https://v2.tauri.app/develop/icons/) for format and bundle configuration details. Native installed-app appearance will be checked when executable builds are available.
