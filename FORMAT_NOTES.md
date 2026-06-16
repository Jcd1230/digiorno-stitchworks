# Format notes encoded by this draft

The generator is based on the empirical Python prototype and the sample SHV files inspected in the conversation.

## SHV coordinate conventions

- Ink/Stitch JSON default: `+X` right, `+Y` down.
- Internal model: `+X` right, `+Y` up.
- SHV raw stitch stream: `+X` right, `+Y` down.

The conversion is therefore:

```text
JSON y down -> internal y up -> SHV raw y down
```

## SHV output layout

```text
0x0000..0x0055  86-byte signature/notice region
0x0056          one-byte design name length
0x0057..        ASCII design name
var             6-byte preview header: height, width, h/2, w/2, h/2, w/2
var             4bpp preview bitmap, high nibble first
var             summary block: color count, constants, bbox, total record count
var             14-byte color rows
var             2-byte-record stitch stream
```

## Stitch encoding

- Normal stitches are signed 8-bit `(dx_raw, dy_raw)` pairs.
- `0x80` is reserved as an escape byte, so normal deltas are restricted to `-127..=127`.
- Large normal stitch deltas are split into multiple legal signed-byte records.
- Jumps/trims are encoded as:

```text
80 01 [signed BE i16 dx] [signed BE i16 dy] 80 02
```

This consumes four 2-byte records and represents one lifted jump event.

## Validation rule

The readback validator requires:

- summary total records == stitch stream bytes / 2,
- summary extents == extents computed from decoded generated stream,
- final decoded position == origin.
