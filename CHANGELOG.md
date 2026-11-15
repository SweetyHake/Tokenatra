# Changelog

## v26.5.3 (2026-11-15)

- Added `xray` and `grid` diagnostic overlays to `tokenatra_preview` and `tokenatra_inspect` (teal checkerboard, hot-pink cut ghost, ring radii `r_in=343` / `r_out=468`, `y=564` preset line, and `-512..1536` coordinate grid).
- Added `polygon` shape (`4x` subpixel AA + optional `feather`) and strict shape validation to `tokenatra_edit_mask`, with full JSON Schema for `shapes` items.
- Upgraded `protect: true` ring mask and `preset1` composition from binary thresholding to continuous 8-bit anti-aliased alpha for smooth rim cuts without stair-stepping.
- Added single-call `tokenatra_pipeline` MCP tool (`POST /agent/v1/pipeline`) combining session setup, conditional `remove_bg`, portrait export, transform, preset, mask edits, effects, token/subject exports, and preview in one request.
- Fixed `example.png` head framing guide coordinates in MCP recipe and preview metadata (`y = -76..199` in `1x` ring coordinates).
- Updated Windows installer to stop running `Tokenatra.exe` and `TokenatraMCP.exe` processes automatically before upgrading.

## v26.5.2 (2026-11-13)

- Added Foundry VTT ring import and updated built-in `cs-rings` ring set.
- Fixed DirectML GPU runtime packaging on Windows and system tray window restore.

## v26.5.1

- Added native Rust MCP server (`TokenatraMCP`) with automatic registration for Claude Desktop, Cursor, Antigravity, and OpenCode.
