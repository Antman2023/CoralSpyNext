# Application icon

`coralspynext.svg` is original artwork created for this project and distributed
under its MIT license. It does not reuse the legacy CoralSpy executable's artwork.
The compact window and coral inspection reticle remain recognizable at small sizes.

- `coralspynext.ico`: transparent 32-bit PNG frames at 16, 24, 32, 48, 64, 128,
  and 256 pixels. Windows uses these for the executable and notification area.
- `coralspynext.png`: the exact 256-pixel ICO frame, used by eframe for the main
  and auxiliary window/taskbar icons.
- `app.rc`: embeds the icon as resource 101, alongside the manifest and version.

To regenerate after editing the SVG, install Inkscape and run:

```sh
python3 scripts/generate-icons.py
python3 scripts/generate-icons.py --check
```

These are authoring commands. Normal builds do not need an SVG renderer or Python.
The integration tests verify every ICO directory entry, PNG dimensions, and the
exact match between the runtime PNG and the 256-pixel executable icon frame.

Windows 11 visual acceptance still requires checking Explorer, the title bar,
Alt+Tab, the taskbar, child windows, and the tray at 100%, 150%, and 200% scaling.
