# Designer 1 SHV Tools

Experimental Rust tools for converting Ink/Stitch JSON exports into Husqvarna/Viking Designer 1 `.SHV` design files.

This is a draft project scaffold. It ports the Python proof-of-concept logic into a modular Rust library, then exposes it through both a modern CLI and an `egui` desktop app.

## What it currently does

- Loads Ink/Stitch JSON with `threadlist`, `extras.name`, and `stitches` rows shaped like `[x, y, "STITCH" | "JUMP" | "END"]`.
- Normalizes Ink/Stitch/SVG `+Y down` coordinates into internal Cartesian `+Y up` coordinates.
- Writes a Designer 1 `.SHV`-like file using the empirically decoded layout.
- Renders a simple 4bpp embedded thumbnail.
- Read-back validates generated SHV bytes:
  - record count equals stitch stream length / 2
  - summary extents match decoded extents
  - final parsed position returns to origin
- Provides a native `egui` UI for loading JSON, previewing stitches, and exporting SHV.
- Provides CLI commands for conversion, inspection, SVG preview, and SHV validation.

## Project layout

```text
src/
  lib.rs                  Shared library entry point
  model.rs                Shared domain model and options
  inkstitch.rs            Ink/Stitch JSON parser and coordinate normalization
  preview.rs              4bpp preview bitmap and SVG preview rendering
  shv.rs                  SHV writer/readback validator
  bin/designer1-cli.rs    CLI frontend
  bin/designer1-gui.rs    egui desktop frontend
```

## Build

```bash
cargo build
cargo build --release
```

## CLI examples

Convert JSON to SHV:

```bash
cargo run --bin designer1-cli -- convert cinnamom.json \
  --output CINNAMOM.SHV \
  --signature official \
  --preview-width 96 \
  --preview-height 24 \
  --validation-report cinnamom-readback.json
```

Inspect normalized JSON stats:

```bash
cargo run --bin designer1-cli -- inspect cinnamom.json
```

Create an SVG preview of the normalized stitch path:

```bash
cargo run --bin designer1-cli -- preview-svg cinnamom.json --output cinnamom-preview.svg
```

Validate/read back a generated SHV:

```bash
cargo run --bin designer1-cli -- validate-shv CINNAMOM.SHV
```

## GUI

```bash
cargo run --bin designer1-gui
```

The GUI lets you:

- open an Ink/Stitch JSON file,
- adjust scale, centering, Y-axis convention, signature mode, and thumbnail size,
- view the normalized stitch path,
- view the embedded SHV thumbnail preview,
- export the generated SHV.

## Important assumptions

- Default input Y-axis is `down`, because Ink/Stitch JSON follows SVG/screen coordinates.
- Internal coordinates are Cartesian: `+X` right, `+Y` up.
- SHV raw stitch stream uses `+Y` down, so SHV raw deltas use `dy_raw = -dy_cartesian`.
- JSON coordinate units are assumed to be close to 0.1 mm units. Use `--scale` if this proves wrong for other exports.
- The SHV thread color index mapping is still provisional. Black maps to observed color index `7`; other colors map to `0` until the palette is decoded.
- This writes only `.SHV`; it does not yet generate matching `.PHV`/`.MHV` disk menu/index files.

## Next likely work

1. Generate/update `MENU_SEL.PHV` and `MENU_XX.MHV` from the same `Design` model.
2. Expand the thread palette mapping.
3. Add import support for VP3 or DST after the SHV writer has been tested on-machine.
4. Add regression tests with known JSON input and expected parsed SHV metadata.
5. Add an optional project file format for disk layout: folders, design slots, menu labels, and thumbnails.
