use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_VERSION: &str = "2025-06-18";
const HTTP_TIMEOUT_SECS: u64 = 180;
const MCP_SERVER_VERSION: &str = "26.5.2";

const INSTRUCTIONS: &str = "\
Use tools from this server through `execute` under `tools[\"tokenatra\"]`.
MCP for Tokenatra drives the Tokenatra token & portrait studio (Rust + ONNX Runtime).
If Tokenatra is not running yet, the first tool call starts `tokenatra.exe --server` automatically.

How a human creates a token in Tokenatra (follow the `example.png` style guide):
1. Open the image: call `tokenatra_create_session({ image_path: \"C:/path/to/art.webp\" })`.
   It returns `session_id` and `source_info` (`width`, `height`, `opaque_pct`, `already_transparent`, `fit1024_bbox`).
   The default ring `\"rune-steel\"` and ring protection mask are already active, just like in the GUI.
2. Remove background ONLY if needed: check `already_transparent` (or `opaque_pct < 92`).
   - If the background is opaque (`already_transparent: false`), call `tokenatra_remove_background({ session_id })`.
   - NEVER call `tokenatra_remove_background` on an already transparent cutout/subject — re-matting eats fine edges and glow.
3. Frame the character (`tokenatra_set_transform`) using the `example.png` overlay (`tokenatra_preview({ session_id, example: true })`):
   In the GUI, the human turns on the \"Пример\" (`example.png`) overlay (at `2x` scale) and aligns the character to its three purple markers:
   - **Область головы (Макушка .. Подбородок)**: the character's head (without tall hats or horns) must fit inside the purple oval!
     - **Макушка (Crown of head)**: `y ~ -130` in 1x ring coords (`y ~ 436` in 2048 2x frame) — the top of the head pops out ABOVE the top rim of the ring (`y = 0`).
     - **Подбородок (Chin / Jawline)**: `y ~ 410` in 1x ring coords (`y ~ 711` in 2048 2x frame), with eyes around `y ~ 150-200` and face centered on `x ~ 512`.
   - **Талия / Пояс (Waist / Belt line)**: `y ~ 715` in 1x ring coords (`y ~ 1230` in 2048 2x frame), with the bottom of the torso/skirt reaching the lower inner rim of the ring (`y ~ 865`).
4. Mask the bottom / sides (`tokenatra_apply_preset` or `tokenatra_edit_mask`):
   - Call `tokenatra_apply_preset({ session_id, preset: \"preset1\" })` (or `\"preset2\"` for wide shields/wings/weapons).
     Because `canvas_mask` covers the full `3x` canvas (`-1024..2048`), `preset1` keeps the head/hat/weapons popping out above the ring (`y < 0`) while cleanly erasing everything outside the bottom/lower sides of the ring!
   - Or use `tokenatra_edit_mask` (`target: \"canvas\"` = Pink Mask fixed to ring; `target: \"image\"` = Blue Eraser attached to character).
5. Look at your work with the Example overlay (`tokenatra_preview({ session_id, example: true })`):
   Pass `example: true` to composite `example.png` (\"Макушка\", \"Область головы\", \"Подбородок\", \"Талия / Пояс\") directly over the preview at `2x` scale and return that image to you! Adjust `scale`, `offset_x`, `offset_y` until the head sits right inside the purple oval and the waist hits \"Талия / Пояс\".
6. Export (`tokenatra_export`):
   - Use `scale_mode: \"auto\"` (or `\"2\"` / `\"1\"`): if the head/hat/weapons pop out beyond the 1x ring (`y < 0`), `\"auto\"` exports a `2x` (or `3x`) canvas so the pop-out head/hat/shield is NEVER clipped at the border!
   - If you want a `subject.webp` (cutout without ring, like `Gambit/subject.webp`), enable drop shadow (`tokenatra_set_effects`) and export with `with_ring: false, scale_mode: \"2\", size: 1024` (which produces a `2048x2048` subject).
";

const RECIPE_TEXT: &str = "\
TOKENATRA STUDIO STYLE GUIDE (based on `example.png` at 2x scale)

1. THE `example.png` VISUAL TEMPLATE (HOW HUMANS ALIGN TOKENS IN TOKENATRA):
   - In Tokenatra, the working canvas is 3x the ring size, and the \"Пример\" (`example.png`)
     overlay is drawn at `2x` scale (`2048x2048` when `size=1024`), where the 1x ring occupies
     the central `1024x1024` box (`[512..1536]` in 2048 coords, or `[0..1024]` in 1x logical coords).
   - Call `tokenatra_preview({ session_id, example: true })` (or `tokenatra_inspect` with `include_image: true, example: true`)
     to see your character with the purple `example.png` overlay directly on top!
   - The three purple markers on `example.png` are:
     1) **Область головы (Макушка .. Подбородок)**:
        - The purple oval in the top center (`cx = 524, cy = 134, rx = 362, ry = 274` in 1x logical coords;
          or `y = 436..711, x = 866..1228` in the `2048x2048` 2x frame).
        - **Макушка (Crown of head / hair top, excluding tall hats/horns)**: must touch the top purple line
          (`y ~ -130` in 1x coords, which is ABOVE the upper ring border `y = 0`!).
        - **Подбородок (Chin / Jawline)**: must touch the middle purple line (`y ~ 410` in 1x coords;
          eyes sit around `y ~ 150-200`, face centered at `x ~ 512`).
        - The entire head (hair + face) fits inside the purple oval!
     2) **Талия / Пояс (Waist / Belt line)**:
        - Must align with the bottom purple line (`y ~ 715` in 1x coords, or `y ~ 1230` in 2048 2x coords).
        - The lower torso / hips continue down to the inner bottom rim of the ring (`y ~ 865`), where they are cut by a smooth arc.

2. COORDINATE SYSTEM & TRANSFORM MATH:
   - Logical 1x ring frame is `1024x1024` (`(0,0)` = top-left of the 1x ring, `(512,512)` = ring center).
   - Coordinates outside `[0..1024]` (like `y = -130` for the crown of the head) are in the 2x/3x pop-out zone above/around the ring!
   - Non-square images preserve their aspect ratio automatically (`fit1024` fits the longest side into 1024 px, centered).
   - Transform formula (from source `fit1024` coordinate `f` to 1x logical coordinate `L`):
       L(fx) = 512 + (fx - 512) * scale + offset_x
       L(fy) = 512 + (fy - 512) * scale + offset_y
   - Solving `scale`, `offset_x`, `offset_y` from Crown `(fx_crown, fy_crown)` -> `y = -130` and Chin `(fx_chin, fy_chin)` -> `y = 410`:
       scale    = 540 / (fy_chin - fy_crown)     # 540 = 410 - (-130)
       offset_x = -(fx_face - 512) * scale
       offset_y = 410 - 512 - (fy_chin - 512) * scale

3. MASKING (PRESETS & PINK CANVAS MASK vs BLUE IMAGE ERASER):
   - The Pink Canvas Mask (`target: \"canvas\"`) spans the full `3x` canvas (`-1024..2048` in 1x coords), so pop-out heads, hats, weapons, and shields outside `0..1024` are preserved!
   - **Built-in Presets (`tokenatra_apply_preset`)**:
     - `\"preset1\"`: standard pop-out mask — keeps the entire inner ring circle + the upper pop-out zone for the head/hat/hair, and erases everything outside the bottom and lower sides of the ring.
     - `\"preset2\"`: wide pop-out mask — keeps upper-left and upper-right pop-outs (great for shields, wings, raised weapons) while cutting the bottom below the ring rim.
     - `\"preset3\"`: narrow vertical column pop-out.
   - **Manual Bottom Arc (for `subject.webp` cutouts without a ring, or full-body art)**:
     Call `tokenatra_edit_mask` (`target: \"canvas\"`, `protect: false`):
       1) `op: \"erase\"`,   `shapes_json: '[{\"rect\":{\"x0\":-512,\"y0\":270,\"x1\":1536,\"y1\":1536}}]'`
       2) `op: \"restore\"`, `shapes_json: '[{\"circle\":{\"cx\":512,\"cy\":270,\"r\":595}}]'`  (arc bottom at `y = 865`, matching the inner ring rim)
       3) `op: \"feather\"`, `px: 8`

4. STEP-BY-STEP WORKFLOW:
   1) `tokenatra_create_session({ image_path: \"...\" })` -> check `already_transparent`.
   2) If `already_transparent == false`, call `tokenatra_remove_background({ session_id })`.
   3) Call `tokenatra_set_transform` and `tokenatra_preview({ session_id, example: true })`.
   4) Look at the returned image with the purple `example.png` overlay:
      - Is the head inside the purple oval (\"Область головы\", between \"Макушка\" and \"Челюсть\")?
      - Is the waist on the \"Пояс\" line?
      Adjust `scale`, `offset_x`, `offset_y` (1-2 iterations) until the head fits the oval and the waist sits on \"Пояс\".
   5) Apply mask (`tokenatra_apply_preset` with `\"preset1\"` or `\"preset2\"`, or custom `tokenatra_edit_mask`).
   6) Export (`tokenatra_export`):
      - For a **Token with Ring** where the head/hat/weapons pop out above the ring: use `with_ring: true, scale_mode: \"auto\"` (or `\"2\"`), `size: 1024`.
      - For a **Subject cutout** (`subject.webp` in `STokens`, used by Foundry dynamic ring or series tokens): enable drop shadow (`{\"enabled\":true,\"angle\":135,\"distance\":7,\"blur\":3,\"opacity\":0.4}`) and export with `with_ring: false, scale_mode: \"2\", size: 1024, format: \"webp\"`.
";

fn server_url() -> String {
    std::env::var("TOKENATRA_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7878".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn log_stderr(msg: &str) {
    let _ = writeln!(io::stderr(), "[mcp] {msg}");
    let _ = io::stderr().flush();
}

enum McpError {
    Tool(String),
    AppDown(String),
}

impl McpError {
    fn message(&self) -> &str {
        match self {
            McpError::Tool(s) | McpError::AppDown(s) => s,
        }
    }
}

fn tools_list() -> Value {
    json!([
        {
            "name": "tokenatra_manifest",
            "description": "Get Tokenatra studio status and manifest: app version, GPU/CPU device, ONNX model status, limits, defaults, and available operations. Auto-starts the Tokenatra server if it is not running.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "tokenatra_recipe",
            "description": "Standard human-like workflows & anchor math: (1) Creating a Token with Ring (1x steel ring + pop-out head + bottom protection/arc), (2) Creating a Subject cutout (square transparent bust with bottom arc), (3) Using Pink Canvas Mask vs Blue Image Eraser and built-in presets.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "tokenatra_list_assets",
            "description": "List available token rings (rune-steel, high-noon, armor_2x, eldritch, etc.) and mask presets (preset1, preset2, preset3, plus user presets).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "tokenatra_create_session",
            "description": "Create an editing session from an image file path OR base64 string. You can pass a local file path (e.g. \"W:/Images/art.webp\") in `image_path` OR directly in `image_b64`! Returns session_id, source dimensions, opaque_pct, already_transparent flag, and fit1024_bbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "image_path": {
                        "type": "string",
                        "description": "Local path to the source image (PNG/WebP/JPG/AVIF). Preferred over base64."
                    },
                    "image_b64": {
                        "type": "string",
                        "description": "Either a local file path (e.g. \"C:/.../img.webp\") OR base64 / data:image/...;base64"
                    },
                    "ring": {
                        "type": "string",
                        "description": "Initial ring name (default \"rune-steel\", pass \"\" or \"none\" for no ring)"
                    }
                }
            }
        },
        {
            "name": "tokenatra_remove_background",
            "description": "Remove background with the ONNX model (BEN2), or toggle between original and cutout (`showing_original: true/false`). Check `already_transparent` from create_session/inspect first — do NOT call on already-transparent images.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "edge_blur": { "type": "number", "description": "0..10, default 1" },
                    "showing_original": {
                        "type": "boolean",
                        "description": "If background was already removed, toggle showing original vs cutout without re-running ONNX"
                    }
                }
            }
        },
        {
            "name": "tokenatra_edit_mask",
            "description": "Edit visibility masks in 1024x1024 logical coords. Supports both GUI mask tools via `target`: \"canvas\" (Pink Mask, fixed in 1x token frame, default) or \"image\" (Blue Eraser, in source image coords, moves/scales with subject) or \"both\" (for reset). ops: erase, restore, apply_preset, invert, feather, grow, reset. shapes_json: [{\"circle\":{\"cx\":512,\"cy\":470,\"r\":395,\"feather\":8}}, {\"rect\":{\"x0\":0,\"y0\":470,\"x1\":1024,\"y1\":1024}}, {\"stroke\":{\"points\":[[200,800],[512,880],[824,800]],\"width\":60,\"soft\":true}}]. When `protect=true` (default), `erase` respects the ring inner circle protection mask so anything inside the ring is safe! Set `protect=false` to cut inside the ring.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "op"],
                "properties": {
                    "session_id": { "type": "string" },
                    "target": {
                        "type": "string",
                        "enum": ["canvas", "image", "both"],
                        "description": "Mask layer: \"canvas\" (pink mask on token frame, default) or \"image\" (blue eraser on subject)"
                    },
                    "op": {
                        "type": "string",
                        "enum": ["erase", "restore", "apply_preset", "invert", "feather", "grow", "reset"]
                    },
                    "shapes_json": {
                        "type": "string",
                        "description": "JSON array of shapes (circle / rect / stroke) in 0..1024 coords"
                    },
                    "preset": { "type": "string", "description": "Preset name for apply_preset (e.g. preset1, preset2, preset3)" },
                    "px": { "type": "number", "description": "Radius in px for feather / grow (e.g. 8)" },
                    "protect": {
                        "type": "boolean",
                        "description": "Respect ring protection mask during erase (default true; pass false for arc cuts inside the ring)"
                    }
                }
            }
        },
        {
            "name": "tokenatra_apply_preset",
            "description": "Apply a named mask preset (e.g. \"preset1\" = top pop-out, \"preset2\" = wide pop-out, \"preset3\" = narrow column pop-out) to the Pink Canvas Mask.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "preset"],
                "properties": {
                    "session_id": { "type": "string" },
                    "preset": { "type": "string" }
                }
            }
        },
        {
            "name": "tokenatra_set_transform",
            "description": "Set subject transform in the 1024x1024 1x token frame: L(f) = 512 + (f - 512) * scale + offset. `scale = 1.0` fits the longest side of the source image into 1024 px (aspect ratio is preserved). `offset_x`, `offset_y` are in 1024 logical px from center (no 3x multiplier!). `rotation` is in degrees clockwise (-180..180).",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "scale": { "type": "number" },
                    "rotation": { "type": "number" },
                    "offset_x": { "type": "number" },
                    "offset_y": { "type": "number" }
                }
            }
        },
        {
            "name": "tokenatra_set_ring",
            "description": "Set token ring by name (e.g. \"rune-steel\", \"high-noon\", \"armor_2x\"), or remove it (name=null or \"none\").",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "name": { "type": "string", "description": "Ring name, or null / \"none\" to remove" }
                }
            }
        },
        {
            "name": "tokenatra_set_effects",
            "description": "Set drop shadow and color correction (just like the GUI accordions). shadow_json: {\"enabled\":true,\"angle\":135,\"distance\":7,\"blur\":3,\"opacity\":0.4}; color_json: {\"enabled\":true,\"saturation\":5,\"lightness\":-5,\"contrast\":0} (percent).",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "shadow_json": { "type": "string" },
                    "color_json": { "type": "string" }
                }
            }
        },
        {
            "name": "tokenatra_history",
            "description": "Undo or redo the last mask/transform/remove_bg operation in the session (up to 30 steps, just like Ctrl+Z / Ctrl+Y in the GUI).",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "action"],
                "properties": {
                    "session_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["undo", "redo"] }
                }
            }
        },
        {
            "name": "tokenatra_portrait",
            "description": "Use the Portrait Generator (square avatar crop, with or without background) and save it to disk. Mirrors the \"Портрет\" panel in the GUI.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "variant": { "type": "string", "enum": ["original", "cutout"] },
                    "scale": { "type": "number" },
                    "rotation": { "type": "number" },
                    "offset_x": { "type": "number" },
                    "offset_y": { "type": "number" },
                    "size": { "type": "number", "description": "512, 1024, 2048, or 4096 (default 1024)" },
                    "format": { "type": "string", "enum": ["webp", "png", "jpg"] },
                    "quality": { "type": "number", "description": "10..100, default 95" },
                    "save_to": { "type": "string", "description": "Target file path (e.g. \"W:/.../portrait.webp\") or folder" }
                }
            }
        },
        {
            "name": "tokenatra_inspect",
            "description": "Inspect current state: `source_info` (width, height, opaque_pct, already_transparent, fit1024_bbox), composed 1x `bbox`, `coverage_pct`, 3x3 grid, `warnings`, and history status. `include_image=true` also returns a visual preview (with optional `example=true` overlay or `guides=true`).",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "include_image": { "type": "boolean" },
                    "preview_size": { "type": "number", "description": "64..1024, default 512" },
                    "example": { "type": "boolean", "description": "Overlay example.png style template (\"Макушка\", \"Область головы\", \"Челюсть\", \"Пояс\")" },
                    "example_opacity": { "type": "number", "description": "0.1..1.0, default 0.65" },
                    "guides": { "type": "boolean", "description": "Draw alignment guides" }
                }
            }
        },
        {
            "name": "tokenatra_preview",
            "description": "Look at the current token (like `look` in Blender MCP). Pass `example=true` to overlay the `example.png` style guide (\"Макушка\", \"Область головы\", \"Челюсть\", \"Пояс\") on a 2x canvas so you can visually verify that the head fits inside the purple oval and the waist aligns with \"Пояс\"!",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "size": { "type": "number", "description": "64..1024, default 512" },
                    "scale_mode": { "type": "string", "enum": ["auto", "1", "2", "3"] },
                    "with_ring": { "type": "boolean", "description": "default true" },
                    "example": { "type": "boolean", "description": "Overlay example.png style template (\"Макушка\", \"Область головы\", \"Челюсть\", \"Пояс\")" },
                    "example_scale": { "type": "number", "description": "1, 2, or 3 (default 2, matching the GUI)" },
                    "example_opacity": { "type": "number", "description": "0.1..1.0, default 0.65" },
                    "guides": { "type": "boolean", "description": "Draw alignment guides on the preview image" }
                }
            }
        },
        {
            "name": "tokenatra_export",
            "description": "Export the final token image (with or without ring). `save_to` accepts EITHER an existing folder path OR a full file path (e.g. \"W:/Images/STokens/Brul/token.png\"). Returns the saved path and attaches the image preview. Default `scale_mode` is \"1\" (full-canvas ring).",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "format": { "type": "string", "enum": ["webp", "png", "jpg"] },
                    "quality": { "type": "number", "description": "10..100, default 95" },
                    "size": { "type": "number", "description": "256..2048, default 1024" },
                    "scale_mode": { "type": "string", "enum": ["auto", "1", "2", "3"] },
                    "with_ring": { "type": "boolean", "description": "true = Save with Ring, false = Save without Ring (subject)" },
                    "save_to": {
                        "type": "string",
                        "description": "Destination file path (e.g. \"C:/.../token.png\") or existing directory path"
                    }
                }
            }
        },
        {
            "name": "tokenatra_close_session",
            "description": "Close a session and free its memory.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": { "session_id": { "type": "string" } }
            }
        }
    ])
}

fn resources_list() -> Value {
    json!([
        {
            "uri": "tokenatra://manifest",
            "name": "Tokenatra manifest",
            "mimeType": "application/json",
            "description": "Limits, defaults, and model status"
        },
        {
            "uri": "tokenatra://assets",
            "name": "Tokenatra assets",
            "mimeType": "application/json",
            "description": "Available token rings and mask presets"
        },
        {
            "uri": "tokenatra://recipe",
            "name": "Tokenatra workflow recipe",
            "mimeType": "text/plain",
            "description": "Human-parity workflow, anchors, and transform math"
        }
    ])
}

fn find_tokenatra_exe() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(env_exe) = std::env::var("TOKENATRA_EXE")
        && !env_exe.trim().is_empty()
    {
        candidates.push(PathBuf::from(env_exe));
    }

    if let Ok(cur) = std::env::current_exe() {
        let stem = cur
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if stem == "tokenatra" {
            candidates.push(cur.clone());
        }
        if let Some(dir) = cur.parent() {
            candidates.push(dir.join("Tokenatra.exe"));
            candidates.push(dir.join("tokenatra.exe"));
            candidates.push(dir.join("tokenatra"));
            if let Some(p1) = dir.parent() {
                candidates.push(p1.join("Tokenatra.exe"));
                candidates.push(p1.join("tokenatra.exe"));
                candidates.push(p1.join("rust").join("target").join("release").join("tokenatra.exe"));
                if let Some(p2) = p1.parent() {
                    candidates.push(p2.join("rust").join("target").join("release").join("tokenatra.exe"));
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(pf) = std::env::var("ProgramFiles") {
            candidates.push(PathBuf::from(pf).join("Tokenatra").join("Tokenatra.exe"));
        }
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(pf86).join("Tokenatra").join("Tokenatra.exe"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Tokenatra")
                    .join("Tokenatra.exe"),
            );
        }
        candidates.push(PathBuf::from(r"Z:\Tokenatra\rust\target\release\tokenatra.exe"));
    }

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/Applications/Tokenatra.app/Contents/MacOS/tokenatra"));
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                PathBuf::from(home).join("Applications/Tokenatra.app/Contents/MacOS/tokenatra"),
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/bin/tokenatra"));
        candidates.push(PathBuf::from("/usr/bin/tokenatra"));
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(PathBuf::from(home).join(".local/bin/tokenatra"));
        }
    }

    candidates.into_iter().find(|p| p.is_file())
}

fn ensure_server_running() -> bool {
    let Some(exe) = find_tokenatra_exe() else {
        return false;
    };
    log_stderr(&format!("Auto-starting Tokenatra server: {} --server", exe.display()));

    let mut cmd = Command::new(&exe);
    cmd.arg("--server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    if let Err(e) = cmd.spawn() {
        log_stderr(&format!("Failed to spawn tokenatra: {e}"));
        return false;
    }

    let ping_url = format!("{}/agent/v1/ping", server_url());
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(resp) = client
            .get(&ping_url)
            .header("X-Tokenatra-Agent", "1")
            .send()
            && resp.status().is_success()
        {
            log_stderr(&format!("Tokenatra server is up at {}", server_url()));
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn http_request_bytes(method: &str, path: &str, body: Option<&Value>) -> Result<(Vec<u8>, String), McpError> {
    let url = format!("{}{}", server_url(), path);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| McpError::AppDown(format!("HTTP client error: {e}")))?;

    for attempt in 0..2 {
        let req_method = match method {
            "POST" => reqwest::Method::POST,
            "DELETE" => reqwest::Method::DELETE,
            _ => reqwest::Method::GET,
        };
        let mut req = client
            .request(req_method, &url)
            .header("X-Tokenatra-Agent", "1");
        if let Some(b) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(serde_json::to_vec(b).unwrap_or_default());
        }

        match req.send() {
            Ok(resp) => {
                let status = resp.status();
                let ctype = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let bytes = resp
                    .bytes()
                    .map(|b| b.to_vec())
                    .unwrap_or_default();
                if !status.is_success() {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    let err_val = serde_json::from_str::<Value>(&text)
                        .unwrap_or_else(|_| json!({ "error": text, "http": status.as_u16() }));
                    return Err(McpError::Tool(err_val.to_string()));
                }
                return Ok((bytes, ctype));
            }
            Err(e) => {
                if attempt == 0 && ensure_server_running() {
                    continue;
                }
                return Err(McpError::AppDown(format!(
                    "Tokenatra is not running at {} ({e}) and could not be auto-started. Launch Tokenatra or `tokenatra.exe --server`.",
                    server_url()
                )));
            }
        }
    }
    Err(McpError::AppDown("Unreachable".into()))
}

fn http_call_json(method: &str, path: &str, body: Option<&Value>) -> Result<Value, McpError> {
    let (bytes, _) = http_request_bytes(method, path, body)?;
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|e| McpError::Tool(format!("Invalid JSON from server: {e}")))
}

fn http_call_raw(method: &str, path: &str, body: Option<&Value>) -> Result<Vec<u8>, McpError> {
    let (bytes, _) = http_request_bytes(method, path, body)?;
    Ok(bytes)
}

fn looks_like_file_path(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.starts_with("data:image/") || s.len() > 1024 || s.contains('\n') {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'/' || bytes[2] == b'\\') {
        return true;
    }
    if s.starts_with("\\\\") || s.starts_with('/') || s.starts_with("./") || s.starts_with("../") {
        return true;
    }
    let lower = s.to_lowercase();
    for ext in [".png", ".webp", ".jpg", ".jpeg", ".avif", ".bmp", ".gif"] {
        if lower.ends_with(ext) {
            return true;
        }
    }
    Path::new(s).is_file()
}

fn resolve_clean_path(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn shapes_from(args: &Value) -> Result<Option<Value>, McpError> {
    if let Some(arr) = args.get("shapes")
        && arr.is_array()
    {
        return Ok(Some(arr.clone()));
    }
    let Some(raw) = args.get("shapes_json").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let parsed: Value = serde_json::from_str(raw)
        .map_err(|e| McpError::Tool(format!("shapes_json is not valid JSON: {e}")))?;
    if !parsed.is_array() {
        return Err(McpError::Tool("shapes_json must be a JSON array of shapes".into()));
    }
    Ok(Some(parsed))
}

fn block_from(args: &Value, key: &str) -> Result<Option<Value>, McpError> {
    let Some(raw) = args.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    if raw.is_object() {
        return Ok(Some(raw.clone()));
    }
    if let Some(s) = raw.as_str() {
        if s.trim().is_empty() {
            return Ok(None);
        }
        let parsed: Value = serde_json::from_str(s)
            .map_err(|e| McpError::Tool(format!("{key} is not valid JSON: {e}")))?;
        if !parsed.is_object() {
            return Err(McpError::Tool(format!("{key} must be a JSON object")));
        }
        return Ok(Some(parsed));
    }
    Ok(None)
}

fn mime_for(fmt: &str) -> &'static str {
    match fmt.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/webp",
    }
}

fn data_url_block(data_url: &str) -> Value {
    let (header, b64) = data_url.split_once(',').unwrap_or(("", data_url));
    let mime = if let Some(rest) = header.strip_prefix("data:")
        && let Some((m, _)) = rest.split_once(';')
    {
        m
    } else {
        "image/webp"
    };
    json!({
        "data": b64,
        "mimeType": mime
    })
}

fn timestamp_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

fn save_export(payload: &[u8], fmt: &str, save_to: Option<&str>, prefix: &str) -> Result<String, McpError> {
    let folder = if let Some(s) = save_to.map(str::trim).filter(|s| !s.is_empty()) {
        let target = Path::new(s);
        let ext = target
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if matches!(ext.as_str(), "png" | "webp" | "jpg" | "jpeg") && !target.is_dir() {
            if let Some(parent) = target.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent)
                    .map_err(|e| McpError::Tool(format!("failed to create parent dir: {e}")))?;
            }
            std::fs::write(target, payload)
                .map_err(|e| McpError::Tool(format!("failed to write {}: {e}", target.display())))?;
            return Ok(target.to_string_lossy().to_string());
        }
        if !target.exists() {
            std::fs::create_dir_all(target)
                .map_err(|e| McpError::Tool(format!("failed to create directory {}: {e}", target.display())))?;
        }
        if !target.is_dir() {
            return Err(McpError::Tool(format!(
                "save_to is neither a valid image file path nor a directory: {s}"
            )));
        }
        target.to_path_buf()
    } else {
        std::env::temp_dir()
    };

    let path = folder.join(format!("{prefix}-{}.{fmt}", timestamp_string()));
    std::fs::write(&path, payload)
        .map_err(|e| McpError::Tool(format!("failed to write {}: {e}", path.display())))?;
    Ok(path.to_string_lossy().to_string())
}

struct ToolOutput {
    text: Option<String>,
    image: Option<Value>,
}

fn req_sid(args: &Value) -> Result<&str, McpError> {
    args.get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Tool("missing parameter 'session_id'".into()))
}

fn call_tool(name: &str, args: &Value) -> Result<ToolOutput, McpError> {
    match name {
        "tokenatra_manifest" => {
            let res = http_call_json("GET", "/agent/v1/manifest", None)?;
            Ok(ToolOutput {
                text: Some(serde_json::to_string_pretty(&res).unwrap_or_default()),
                image: None,
            })
        }
        "tokenatra_recipe" => Ok(ToolOutput {
            text: Some(RECIPE_TEXT.to_string()),
            image: None,
        }),
        "tokenatra_list_assets" => {
            let res = http_call_json("GET", "/agent/v1/assets", None)?;
            Ok(ToolOutput {
                text: Some(serde_json::to_string_pretty(&res).unwrap_or_default()),
                image: None,
            })
        }
        "tokenatra_create_session" => {
            let mut body = serde_json::Map::new();
            let img_path = args
                .get("image_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let img_b64 = args
                .get("image_b64")
                .or_else(|| args.get("image"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());

            if let Some(p_str) = img_path {
                let p = Path::new(p_str);
                if !p.is_file() {
                    return Err(McpError::Tool(format!("image_path does not exist: {p_str}")));
                }
                body.insert("image_path".into(), Value::String(resolve_clean_path(p)));
            } else if let Some(b64_or_path) = img_b64 {
                if looks_like_file_path(b64_or_path) {
                    let p = Path::new(b64_or_path);
                    if !p.is_file() {
                        return Err(McpError::Tool(format!("file does not exist: {b64_or_path}")));
                    }
                    body.insert("image_path".into(), Value::String(resolve_clean_path(p)));
                } else {
                    body.insert("image".into(), Value::String(b64_or_path.to_string()));
                }
            } else {
                return Err(McpError::Tool(
                    "provide image_path (local file path) or image_b64".into(),
                ));
            }

            if let Some(r_val) = args.get("ring") {
                if r_val.is_null() {
                    body.insert("ring".into(), Value::Null);
                } else if let Some(r_str) = r_val.as_str() {
                    let trimmed = r_str.trim().to_lowercase();
                    if trimmed.is_empty() || trimmed == "none" || trimmed == "null" {
                        body.insert("ring".into(), Value::Null);
                    } else {
                        body.insert("ring".into(), Value::String(r_str.to_string()));
                    }
                }
            }

            let res = http_call_json("POST", "/agent/v1/sessions", Some(&Value::Object(body)))?;
            Ok(ToolOutput {
                text: Some(serde_json::to_string_pretty(&res).unwrap_or_default()),
                image: None,
            })
        }
        "tokenatra_remove_background" => {
            let sid = req_sid(args)?;
            let mut body = serde_json::Map::new();
            let edge_blur = args.get("edge_blur").and_then(|v| v.as_f64()).unwrap_or(1.0);
            body.insert("edge_blur".into(), json!(edge_blur));
            if let Some(so) = args.get("showing_original").and_then(|v| v.as_bool()) {
                body.insert("showing_original".into(), Value::Bool(so));
            }
            let res = http_call_json(
                "POST",
                &format!("/agent/v1/sessions/{sid}/remove_bg"),
                Some(&Value::Object(body)),
            )?;
            Ok(ToolOutput {
                text: Some(serde_json::to_string_pretty(&res).unwrap_or_default()),
                image: None,
            })
        }
        "tokenatra_edit_mask" => {
            let sid = req_sid(args)?;
            let op = args
                .get("op")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::Tool("missing parameter 'op'".into()))?;
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("canvas");
            let px = args.get("px").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let protect = args.get("protect").and_then(|v| v.as_bool()).unwrap_or(true);

            let mut body = serde_json::Map::new();
            body.insert("op".into(), Value::String(op.to_string()));
            body.insert("target".into(), Value::String(target.to_string()));
            body.insert("px".into(), json!(px));
            body.insert("protect".into(), Value::Bool(protect));
            if let Some(shapes) = shapes_from(args)? {
                body.insert("shapes".into(), shapes);
            }
            if let Some(preset) = args.get("preset").and_then(|v| v.as_str()) {
                body.insert("preset".into(), Value::String(preset.to_string()));
            }
            let res = http_call_json(
                "POST",
                &format!("/agent/v1/sessions/{sid}/mask"),
                Some(&Value::Object(body)),
            )?;
            Ok(ToolOutput {
                text: Some(res.to_string()),
                image: None,
            })
        }
        "tokenatra_apply_preset" => {
            let sid = req_sid(args)?;
            let preset = args
                .get("preset")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::Tool("missing parameter 'preset'".into()))?;
            let body = json!({ "op": "apply_preset", "preset": preset });
            let res = http_call_json("POST", &format!("/agent/v1/sessions/{sid}/mask"), Some(&body))?;
            Ok(ToolOutput {
                text: Some(res.to_string()),
                image: None,
            })
        }
        "tokenatra_set_transform" => {
            let sid = req_sid(args)?;
            let mut body = serde_json::Map::new();
            for k in ["scale", "rotation", "offset_x", "offset_y", "fit_zone"] {
                if let Some(v) = args.get(k)
                    && !v.is_null()
                {
                    body.insert(k.to_string(), v.clone());
                }
            }
            let res = http_call_json(
                "POST",
                &format!("/agent/v1/sessions/{sid}/transform"),
                Some(&Value::Object(body)),
            )?;
            Ok(ToolOutput {
                text: Some(res.to_string()),
                image: None,
            })
        }
        "tokenatra_set_ring" => {
            let sid = req_sid(args)?;
            let ring_val = match args.get("name").and_then(|v| v.as_str()) {
                Some(s) if !matches!(s.trim().to_lowercase().as_str(), "" | "none" | "null") => {
                    Value::String(s.to_string())
                }
                _ => Value::Null,
            };
            let body = json!({ "name": ring_val });
            let res = http_call_json("POST", &format!("/agent/v1/sessions/{sid}/ring"), Some(&body))?;
            Ok(ToolOutput {
                text: Some(res.to_string()),
                image: None,
            })
        }
        "tokenatra_set_effects" => {
            let sid = req_sid(args)?;
            let mut body = serde_json::Map::new();
            let shadow = block_from(args, "shadow_json")?.or(block_from(args, "shadow")?);
            let color = block_from(args, "color_json")?.or(block_from(args, "color")?);
            if let Some(s) = shadow {
                body.insert("shadow".into(), s);
            }
            if let Some(c) = color {
                body.insert("color".into(), c);
            }
            let res = http_call_json(
                "POST",
                &format!("/agent/v1/sessions/{sid}/effects"),
                Some(&Value::Object(body)),
            )?;
            Ok(ToolOutput {
                text: Some(res.to_string()),
                image: None,
            })
        }
        "tokenatra_history" => {
            let sid = req_sid(args)?;
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("undo");
            let body = json!({ "action": action });
            let res = http_call_json("POST", &format!("/agent/v1/sessions/{sid}/history"), Some(&body))?;
            Ok(ToolOutput {
                text: Some(res.to_string()),
                image: None,
            })
        }
        "tokenatra_portrait" => {
            let sid = req_sid(args)?;
            let mut body = serde_json::Map::new();
            for k in ["variant", "scale", "rotation", "offset_x", "offset_y", "size", "format", "quality"] {
                if let Some(v) = args.get(k)
                    && !v.is_null()
                {
                    body.insert(k.to_string(), v.clone());
                }
            }
            let fmt = body
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("webp")
                .to_string();
            let payload = http_call_raw(
                "POST",
                &format!("/agent/v1/sessions/{sid}/portrait"),
                Some(&Value::Object(body)),
            )?;
            let save_to = args
                .get("save_to")
                .or_else(|| args.get("output_path"))
                .and_then(|v| v.as_str());
            let path = save_export(&payload, &fmt, save_to, "tokenatra-portrait")?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
            Ok(ToolOutput {
                text: Some(format!("Saved portrait: {path}")),
                image: Some(json!({ "data": b64, "mimeType": mime_for(&fmt) })),
            })
        }
        "tokenatra_inspect" => {
            let sid = req_sid(args)?;
            let mut qs: Vec<String> = Vec::new();
            if args.get("include_image").and_then(|v| v.as_bool()).unwrap_or(false) {
                qs.push("include_image=1".into());
            }
            if let Some(sz) = args.get("preview_size").and_then(|v| v.as_i64()) {
                qs.push(format!("preview_size={sz}"));
            }
            if args.get("example").and_then(|v| v.as_bool()).unwrap_or(false) {
                qs.push("example=1".into());
            }
            if let Some(op) = args.get("example_opacity").and_then(|v| v.as_f64()) {
                qs.push(format!("example_opacity={op}"));
            }
            if args.get("guides").and_then(|v| v.as_bool()).unwrap_or(false) {
                qs.push("guides=1".into());
            }
            if let Some(sm) = args.get("scale_mode").and_then(|v| v.as_str()) {
                qs.push(format!("scale_mode={sm}"));
            }
            let suffix = if qs.is_empty() {
                String::new()
            } else {
                format!("?{}", qs.join("&"))
            };
            let mut res = http_call_json("GET", &format!("/agent/v1/sessions/{sid}/inspect{suffix}"), None)?;
            let image_block = res
                .as_object_mut()
                .and_then(|obj| obj.remove("image_data_url"))
                .and_then(|v| v.as_str().map(data_url_block));
            Ok(ToolOutput {
                text: Some(serde_json::to_string_pretty(&res).unwrap_or_default()),
                image: image_block,
            })
        }
        "tokenatra_preview" => {
            let sid = req_sid(args)?;
            let size = args.get("size").and_then(|v| v.as_i64()).unwrap_or(512);
            let example = args.get("example").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut qs = vec![format!("size={size}")];
            if let Some(sm) = args.get("scale_mode").and_then(|v| v.as_str()) {
                qs.push(format!("scale_mode={sm}"));
            } else if example {
                qs.push("scale_mode=2".into());
            } else {
                qs.push("scale_mode=1".into());
            }
            if let Some(wr) = args.get("with_ring").and_then(|v| v.as_bool()) {
                qs.push(format!("with_ring={}", if wr { "1" } else { "0" }));
            }
            if example {
                qs.push("example=1".into());
            }
            if let Some(es) = args.get("example_scale").and_then(|v| v.as_i64()) {
                qs.push(format!("example_scale={es}"));
            }
            if let Some(eo) = args.get("example_opacity").and_then(|v| v.as_f64()) {
                qs.push(format!("example_opacity={eo}"));
            }
            if args.get("guides").and_then(|v| v.as_bool()).unwrap_or(false) {
                qs.push("guides=1".into());
            }
            let payload = http_call_raw(
                "GET",
                &format!("/agent/v1/sessions/{sid}/preview?{}", qs.join("&")),
                None,
            )?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
            Ok(ToolOutput {
                text: None,
                image: Some(json!({ "data": b64, "mimeType": "image/webp" })),
            })
        }
        "tokenatra_export" => {
            let sid = req_sid(args)?;
            let mut body = serde_json::Map::new();
            for k in ["format", "quality", "size", "scale_mode", "with_ring"] {
                if let Some(v) = args.get(k)
                    && !v.is_null()
                {
                    body.insert(k.to_string(), v.clone());
                }
            }
            if !body.contains_key("scale_mode") {
                body.insert("scale_mode".into(), Value::String("1".into()));
            }
            let save_to = args
                .get("output_path")
                .or_else(|| args.get("save_to"))
                .and_then(|v| v.as_str());
            if let Some(st) = save_to
                && !body.contains_key("format")
            {
                let ext = Path::new(st)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default()
                    .to_lowercase();
                if matches!(ext.as_str(), "png" | "webp" | "jpg" | "jpeg") {
                    let fmt = if ext == "jpeg" { "jpg" } else { ext.as_str() };
                    body.insert("format".into(), Value::String(fmt.to_string()));
                }
            }
            let fmt = body
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("png")
                .to_string();
            body.insert("format".into(), Value::String(fmt.clone()));

            let payload = http_call_raw(
                "POST",
                &format!("/agent/v1/sessions/{sid}/export"),
                Some(&Value::Object(body)),
            )?;
            let path = save_export(&payload, &fmt, save_to, "tokenatra-export")?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
            Ok(ToolOutput {
                text: Some(format!("Saved: {path}")),
                image: Some(json!({ "data": b64, "mimeType": mime_for(&fmt) })),
            })
        }
        "tokenatra_close_session" => {
            let sid = req_sid(args)?;
            let _ = http_call_json("DELETE", &format!("/agent/v1/sessions/{sid}"), None)?;
            Ok(ToolOutput {
                text: Some("closed".into()),
                image: None,
            })
        }
        _ => Err(McpError::Tool(format!("unknown tool {name}"))),
    }
}

fn send_json(msg: &Value) {
    if let Ok(line) = serde_json::to_string(msg) {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

fn send_result(req_id: &Value, value: Value) {
    send_json(&json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": value
    }));
}

fn send_error(req_id: &Value, code: i64, message: &str) {
    send_json(&json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "error": { "code": code, "message": message }
    }));
}

fn handle_message(msg: &Value) {
    let Some(obj) = msg.as_object() else { return };
    let method = obj.get("method").and_then(|v| v.as_str()).unwrap_or_default();
    let req_id = obj.get("id");
    let is_notification = !obj.contains_key("id");

    if method.starts_with("notifications/") {
        return;
    }
    let Some(id_val) = req_id else {
        return;
    };
    if id_val.is_null()
        && matches!(
            method,
            "initialize" | "ping" | "tools/list" | "tools/call" | "resources/list" | "resources/read"
        )
    {
        return;
    }

    match method {
        "initialize" => {
            let requested = obj
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(LATEST_VERSION);
            let version = if SUPPORTED_VERSIONS.contains(&requested) {
                requested
            } else {
                LATEST_VERSION
            };
            send_result(
                id_val,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {}, "resources": {} },
                    "serverInfo": { "name": "tokenatra", "version": MCP_SERVER_VERSION },
                    "instructions": INSTRUCTIONS
                }),
            );
        }
        "ping" => {
            send_result(id_val, json!({}));
        }
        "tools/list" => {
            send_result(id_val, json!({ "tools": tools_list() }));
        }
        "tools/call" => {
            let params = obj.get("params").unwrap_or(&Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let args = params.get("arguments").unwrap_or(&Value::Null);
            match call_tool(name, args) {
                Ok(out) => {
                    let mut content: Vec<Value> = Vec::new();
                    if let Some(t) = out.text {
                        content.push(json!({ "type": "text", "text": t }));
                    }
                    if let Some(img) = out.image {
                        let mut img_obj = serde_json::Map::new();
                        img_obj.insert("type".into(), Value::String("image".into()));
                        if let Some(map) = img.as_object() {
                            for (k, v) in map {
                                img_obj.insert(k.clone(), v.clone());
                            }
                        }
                        content.push(Value::Object(img_obj));
                    }
                    send_result(id_val, json!({ "content": content }));
                }
                Err(e) => {
                    send_result(
                        id_val,
                        json!({
                            "content": [{ "type": "text", "text": e.message() }],
                            "isError": true
                        }),
                    );
                }
            }
        }
        "resources/list" => {
            send_result(id_val, json!({ "resources": resources_list() }));
        }
        "resources/read" => {
            let uri = obj
                .get("params")
                .and_then(|p| p.get("uri"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if uri == "tokenatra://recipe" {
                send_result(
                    id_val,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "text/plain",
                            "text": RECIPE_TEXT
                        }]
                    }),
                );
                return;
            }
            let route = match uri {
                "tokenatra://manifest" => "/agent/v1/manifest",
                "tokenatra://assets" => "/agent/v1/assets",
                _ => {
                    send_error(id_val, -32602, &format!("unknown resource {uri}"));
                    return;
                }
            };
            match http_call_json("GET", route, None) {
                Ok(data) => {
                    send_result(
                        id_val,
                        json!({
                            "contents": [{
                                "uri": uri,
                                "mimeType": "application/json",
                                "text": serde_json::to_string_pretty(&data).unwrap_or_default()
                            }]
                        }),
                    );
                }
                Err(e) => {
                    send_error(id_val, -32603, e.message());
                }
            }
        }
        _ => {
            if !is_notification {
                send_error(id_val, -32601, &format!("method not found: {method}"));
            }
        }
    }
}

#[allow(dead_code)]
pub fn run_stdio() -> i32 {
    log_stderr(&format!("tokenatra-mcp v{MCP_SERVER_VERSION} -> {}", server_url()));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(val) => handle_message(&val),
            Err(e) => log_stderr(&format!("bad json line: {e}")),
        }
    }
    0
}

// ===== Automatic MCP Registration for AI Clients =====

/// Resolves the best command + args to invoke the Tokenatra MCP server.
/// Prefers `TokenatraMCP.exe` (console binary) if present next to the current exe,
/// otherwise falls back to `Tokenatra.exe --mcp`.
#[allow(dead_code)]
pub fn mcp_command_and_args() -> (String, Vec<String>) {
    if let Ok(cur) = std::env::current_exe() {
        let cur_clean = resolve_clean_path(&cur);
        let stem = cur
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if stem == "tokenatramcp" || stem == "tokenatra-mcp" {
            return (cur_clean, Vec::new());
        }
        if let Some(dir) = cur.parent() {
            #[cfg(windows)]
            let mcp_bin = dir.join("TokenatraMCP.exe");
            #[cfg(not(windows))]
            let mcp_bin = dir.join("TokenatraMCP");
            if mcp_bin.is_file() {
                return (resolve_clean_path(&mcp_bin), Vec::new());
            }
        }
        return (cur_clean, vec!["--mcp".to_string()]);
    }
    ("TokenatraMCP.exe".to_string(), Vec::new())
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn upsert_mcp_servers_file(config_path: &Path, cmd: &str, args: &[String], create_parent: bool) -> bool {
    let Some(parent) = config_path.parent() else {
        return false;
    };
    if !parent.exists() && (!create_parent || std::fs::create_dir_all(parent).is_err()) {
        return false;
    }
    let mut root: Value = if config_path.is_file() {
        let Ok(raw) = std::fs::read_to_string(config_path) else {
            return false;
        };
        if raw.trim().is_empty() {
            json!({})
        } else {
            let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
                return false;
            };
            parsed
        }
    } else {
        json!({})
    };

    let Some(root_obj) = root.as_object_mut() else {
        return false;
    };
    let servers = root_obj
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let Some(servers_obj) = servers.as_object_mut() else {
        return false;
    };

    let desired = json!({
        "command": cmd,
        "args": args
    });
    if servers_obj.get("tokenatra") == Some(&desired) {
        return true;
    }
    servers_obj.insert("tokenatra".to_string(), desired);

    if let Ok(formatted) = serde_json::to_string_pretty(&root) {
        std::fs::write(config_path, format!("{formatted}\n")).is_ok()
    } else {
        false
    }
}

fn upsert_opencode_file(config_path: &Path, cmd: &str, args: &[String], create_parent: bool) -> bool {
    let Some(parent) = config_path.parent() else {
        return false;
    };
    if !parent.exists() && (!create_parent || std::fs::create_dir_all(parent).is_err()) {
        return false;
    }
    let mut root: Value = if config_path.is_file() {
        let Ok(raw) = std::fs::read_to_string(config_path) else {
            return false;
        };
        if raw.trim().is_empty() {
            json!({ "$schema": "https://opencode.ai/config.json" })
        } else {
            let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
                return false;
            };
            parsed
        }
    } else {
        json!({ "$schema": "https://opencode.ai/config.json" })
    };

    let Some(root_obj) = root.as_object_mut() else {
        return false;
    };
    let mcp = root_obj.entry("mcp").or_insert_with(|| json!({}));
    let Some(mcp_obj) = mcp.as_object_mut() else {
        return false;
    };
    let servers = mcp_obj.entry("servers").or_insert_with(|| json!({}));
    let Some(servers_obj) = servers.as_object_mut() else {
        return false;
    };

    let mut cmd_vec = vec![cmd.to_string()];
    cmd_vec.extend(args.iter().cloned());
    let desired = json!({
        "type": "local",
        "command": cmd_vec
    });
    if servers_obj.get("tokenatra") == Some(&desired) {
        return true;
    }
    servers_obj.insert("tokenatra".to_string(), desired);

    if let Ok(formatted) = serde_json::to_string_pretty(&root) {
        std::fs::write(config_path, format!("{formatted}\n")).is_ok()
    } else {
        false
    }
}

/// Registers Tokenatra MCP in detected AI client configs (Antigravity, Claude Desktop, Cursor, Windsurf, OpenCode).
/// When `force_create_primary` is true (`--install-mcp`), creates `~/.gemini/config/mcp_config.json` even if the folder didn't exist yet.
#[allow(dead_code)]
pub fn register_mcp_clients(force_create_primary: bool) -> Vec<String> {
    let (cmd, args) = mcp_command_and_args();
    let mut updated: Vec<String> = Vec::new();

    if let Some(home) = home_dir() {
        // 1. Google Antigravity (~/.gemini/config/mcp_config.json)
        let agy_cfg = home.join(".gemini").join("config").join("mcp_config.json");
        let agy_exists = home.join(".gemini").exists() || force_create_primary;
        if agy_exists && upsert_mcp_servers_file(&agy_cfg, &cmd, &args, true) {
            updated.push(agy_cfg.to_string_lossy().to_string());
        }

        // 2. Cursor (~/.cursor/mcp.json)
        let cursor_dir = home.join(".cursor");
        let cursor_cfg = cursor_dir.join("mcp.json");
        if (cursor_dir.exists() || cursor_cfg.is_file())
            && upsert_mcp_servers_file(&cursor_cfg, &cmd, &args, false)
        {
            updated.push(cursor_cfg.to_string_lossy().to_string());
        }

        // 3. Windsurf (~/.codeium/windsurf/mcp_config.json)
        let windsurf_dir = home.join(".codeium").join("windsurf");
        let windsurf_cfg = windsurf_dir.join("mcp_config.json");
        if (windsurf_dir.exists() || windsurf_cfg.is_file())
            && upsert_mcp_servers_file(&windsurf_cfg, &cmd, &args, false)
        {
            updated.push(windsurf_cfg.to_string_lossy().to_string());
        }

        // 4. OpenCode (~/.config/opencode/opencode.json)
        let opencode_dir = home.join(".config").join("opencode");
        let opencode_cfg = opencode_dir.join("opencode.json");
        if (opencode_dir.exists() || opencode_cfg.is_file())
            && upsert_opencode_file(&opencode_cfg, &cmd, &args, false)
        {
            updated.push(opencode_cfg.to_string_lossy().to_string());
        }
    }

    // 5. Claude Desktop
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        let claude_dir = PathBuf::from(appdata).join("Claude");
        let claude_cfg = claude_dir.join("claude_desktop_config.json");
        if (claude_dir.exists() || claude_cfg.is_file())
            && upsert_mcp_servers_file(&claude_cfg, &cmd, &args, false)
        {
            updated.push(claude_cfg.to_string_lossy().to_string());
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = home_dir() {
        let claude_dir = home.join("Library/Application Support/Claude");
        let claude_cfg = claude_dir.join("claude_desktop_config.json");
        if (claude_dir.exists() || claude_cfg.is_file())
            && upsert_mcp_servers_file(&claude_cfg, &cmd, &args, false)
        {
            updated.push(claude_cfg.to_string_lossy().to_string());
        }
    }
    #[cfg(target_os = "linux")]
    if let Some(home) = home_dir() {
        let claude_dir = home.join(".config/Claude");
        let claude_cfg = claude_dir.join("claude_desktop_config.json");
        if (claude_dir.exists() || claude_cfg.is_file())
            && upsert_mcp_servers_file(&claude_cfg, &cmd, &args, false)
        {
            updated.push(claude_cfg.to_string_lossy().to_string());
        }
    }

    updated
}
