# Changelog

## v26.5.4 (2026-11-21)

- Added animated image and video support (`GIF`, animated `WebP`, `APNG`, `WebM`, `MP4`, `MOV`, `M4V`, `OGV`, `MKV`, `AVI`) across the token editor, portrait generator, file converter, batch background remover, and Foundry VTT connector, with automatic transparent `WebM` (`VP9`/`VP8` Alpha) export via WebCodecs + built-in EBML muxer (and optional FFmpeg fallback).
- Added playback controls (play/pause, timeline scrubbing, progress overlay with cancel) and frame-by-frame AI background removal for animated tokens.
- Added MCP downgrade guard in `register_mcp_clients` (probes existing binaries via JSON-RPC `initialize` and keeps active dev builds or newer versions unless `--install-mcp` is explicitly passed) and `--version` / `-V` flags for `tokenatra` and `TokenatraMCP`.
- Upgraded default `tokenatra_preview` resolution to `768` (`1536x1536` at `2x`, matching native vision tile grids, WebP `quality=92`) with `2px` X-Ray rim markers, crisp `5x7` coordinate grid labels, sniper macro `crop: [x0, y0, x1, y1]` (with `10px` grid), and `eyes: true` `1:1` eye-gaze crops.
- Added programmatic mask QA linter (`mask.qa` in `tokenatra_inspect` and `tokenatra_pipeline`) to detect `detached_islands`, `bottom_ring_gap`, and horizontal `step_564` cuts with exact `bbox` coordinates.
- Added native batch contact-sheet generator `tokenatra_qa_sheet` (`POST /agent/v1/qa_sheet`) supporting `xray`, `eyes`, and `showcase` modes over active sessions or disk directories (`subjects_dir` + `portraits_dir` + `transforms_json`).
- Added bidirectional Live GUI Sync (`tokenatra_gui_sync`, `POST /agent/v1/gui_sync`, and `static/js/agentSync.js`) for inspecting and tweaking agent sessions live inside the desktop studio without clobbering manual user work.

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
