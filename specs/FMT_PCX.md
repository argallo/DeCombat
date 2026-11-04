# FMT_PCX: PCX images as the 2009 client reads them (the front end's mouse cursor)

- Status: SPEC (public format knowledge: the ZSoft PCX technical reference, version 5; checked against the 2009
  engine's decoder and the user ROM's three `.pcx` entries)
- Modules: `engine` = 2009 EU-test `Engine.exe`, `cshell` = `CShell.dll`; RVAs as `module+0xRVA` of
  ``
- ROM data policy: header facts and counts only. No pixel data is reproduced.

Byte order is little-endian throughout.

## 1. Who reads PCX

| what | evidence |
|---|---|
| the shell loads its cursor image by the path `LOGO\CURSOR0.PCX` (an entry of the logo archive `LOGO.rez`), once, through the engine's client-interface slot +0x12c (takes a path, returns an image handle) | cshell+0x1bf010, cshell+0x1ef3a0 |
| the shell draws it each frame while the hardware cursor is off (option `HardwareCursor` > 0 and `DisableHardwareCursor` 0 turn the hardware cursor on; both default 0) through slot +0x124, at the shell's pointer position, the image's top-left at the pointer | cshell+0x1ef530 |
| the engine's PCX decoder (section 3 limits) | engine+0x50570 |

Other `.pcx` entries of the logo archive (a warning page and a publisher logo) have no reader of their own in the
front end (FMT_SPR_LTC_FX.md §9.4).

## 2. Header (128 bytes at offset 0)

| off | size | meaning | read by the 2009 decoder |
|---|---|---|---|
| 0x00 | u8 | manufacturer tag, 0x0A | not checked |
| 0x01 | u8 | version (5 = with a 256-colour palette) | not checked |
| 0x02 | u8 | encoding, 1 = run-length | not checked (always run-length decoded) |
| 0x03 | u8 | bits per pixel per plane | must be 8 (engine+0x50570) |
| 0x04 | 4 x i16 | window `xmin, ymin, xmax, ymax` (inclusive) | width `xmax - xmin + 1`, height `ymax - ymin + 1` |
| 0x0C | 2 x u16 | resolution (dpi) | no |
| 0x10 | 48 | 16-colour palette (EGA) | no |
| 0x40 | u8 | reserved | no |
| 0x41 | u8 | planes | must be 1 (palette image) or 3 (red, green, blue) |
| 0x42 | u16 | bytes per plane line (even; may exceed the width: padding) | the scan-line stride |
| 0x44 | u16 | palette interpretation | no |
| 0x46 | 2 x u16 | source screen size | no |
| 0x4A | 54 | filler | no |

## 3. Scan lines and runs

The image data starts at offset 128. Each scan line is `planes x bytes_per_line` bytes after decoding: the plane
lines one after the other (for 3 planes: the red line, then the green line, then the blue line). Pixel `x` of the
line is byte `x` of each plane line; the bytes from `width` to `bytes_per_line - 1` are padding.

Run-length coding, byte by byte:

- a byte with its top two bits set (`b & 0xC0 == 0xC0`) is a count `b & 0x3F` (0..63) and the next byte is the value
  repeated that many times;
- any other byte is one literal value. A value of 0xC0 or more must therefore be written as a run (count 1).

Runs do not cross scan lines; the 2009 decoder rejects a file whose run is longer than what is left of its scan line
(engine+0x50570). A run of count 0 writes nothing.

## 4. Palette (1-plane images)

The last 769 bytes of the file: the marker byte 0x0C, then 256 entries of `r, g, b` (one byte each). Pixel values
of a 1-plane image index it. 3-plane images carry no palette (their bytes are the colours themselves).

## 5. Reimplementation notes (DeCombat)

- `dc_tex::decode_pcx` accepts what the 2009 decoder accepts (8 bits per plane, 1 or 3 planes) and additionally
  checks the tag (0x0A), the encoding (1), `bytes_per_line >= width`, a side limit of 4096 (allocation bound on
  hostile input), the palette marker of a 1-plane image, and data that ends before the last line.
- The output is RGBA8, alpha 255 (PCX has no alpha). The cursor's transparency is a colour key the menu applies
  (DC_DEFAULTS CLI-481).

## 6. ROM facts (eu-test, 2026-10-27)

| entry | header | size |
|---|---|---|
| the cursor image | version 5, run-length, 8 bits x 3 planes, 32 bytes per line | 32 x 32 |

The cursor's background is pure black (0, 0, 0) and every pure-black texel lies outside the drawn shape (OBSERVED,
counts only).
