# Tokenatra — AGENTS.md

Инструкции для ИИ-агентов, работающих с этим репозиторием и приложением.

**Приложение** (`rust/`): axum (HTTP) + ort (ONNX Runtime) + wry/tao (GUI).

## Команды

```bat
cd rust
cargo build --release        # сборка (exe: rust\target\release\tokenatra.exe)
cargo run --release          # запуск с GUI
cargo run --release -- --server   # headless: только HTTP :7878
```

Установщик Windows: `build_installer.bat` (cargo + Inno Setup) →
`dist\installer\Tokenatra_v*_windows_x86_64_setup.exe`.
CI: `.github/workflows/rust-build.yml` (3 платформы).

## Архитектура

```
rust/src/
  main.rs            # запуск: окно/трей + spawn_server
  state.rs           # AppState: ONNX-сессия (Mutex), трей, embed-экспорты
  infer.rs           # remove_background (ort): RMBG-2.0 / BEN2 / BiRefNet
  http/
    mod.rs           # роутер + guard_mw (localhost/network/agent) + CSP
    process.rs       # POST /process — remove_bg (multipart)
    agent.rs         # /agent/v1/* — агентский API (см. ниже)
    rings.rs assets.rs presets.rs config_routes.rs files.rs
    embed.rs system.rs window.rs models.rs
  consts.rs paths.rs config.rs win.rs tray.rs updater.rs
```

Фронтенд — общий, `static/` + `templates/index.html` (Rust сам рендерит Jinja-плейсхолдеры).

## Agent API (`/agent/v1/*`)

JSON поверх HTTP, только localhost, заголовок `X-Tokenatra-Agent: 1` (см. guard_mw).

```
GET  /agent/v1/manifest  /assets  /tools_schema
POST /agent/v1/sessions                         {image_path | image: base64|data_url, ring}
GET/DELETE /agent/v1/sessions/{id}
POST /agent/v1/sessions/{id}/remove_bg          {edge_blur, showing_original}
POST /agent/v1/sessions/{id}/mask               {target: canvas|image|both, op, shapes|preset|px, protect}
      op: erase|restore (shapes rect/circle/stroke), apply_preset, invert, feather, grow, reset
POST /agent/v1/sessions/{id}/transform          {scale, rotation, offset_x, offset_y, fit_zone}
POST /agent/v1/sessions/{id}/ring               {name|null}
POST /agent/v1/sessions/{id}/effects            {shadow, color}
POST /agent/v1/sessions/{id}/history            {action: undo|redo}
POST /agent/v1/sessions/{id}/portrait           {variant, scale, rotation, offset_x, offset_y, size, format, quality}
POST /agent/v1/sessions/{id}/save_preset        {name}
GET  /agent/v1/sessions/{id}/preview?size=512&scale_mode=1&guides=1
GET  /agent/v1/sessions/{id}/inspect?include_image=1&guides=1
POST /agent/v1/sessions/{id}/export             {format, quality, size, scale_mode, with_ring}
```

Лимиты: 4 сессии (LRU), TTL 30 мин, 200 операций/сессию, body ≤ 15 МБ,
base dim ≤ 2048, экспорт 256–2048.

**Семантика transform**: логический канвас 1024, `L(f) = 512 + (f−512)·scale + offset`
(f — fit-координата источника). `scale_mode: "1"` — экспорт ровно область кольца 1×
(для стиля «кольцо на весь канвас + голова над кольцом» — ВСЕГДА "1", не "auto").

## Рецепты токенов и subject-файлов (по шаблону `example.png`)

Полное руководство вшито в MCP (`tokenatra_recipe` и `<mcp_instructions>`).

В интерфейсе Tokenatra (и в MCP через `tokenatra_preview({ session_id, example: true })`)
на рабочий холст `2x` (`2048×2048`) накладывается шаблон `example.png` с тремя
пурпурными ориентирами (в логических координатах кольца `1x`, где `0..1024` —
область кольца `1x`, а отрицательные `y` выходят за верхний край кольца в зону `2x`):
- **Область головы (пурпурный овал `cx ≈ 524, cy ≈ 134, rx ≈ 362, ry ≈ 274`)**:
  голова персонажа (без учёта шляпы / рогов) должна целиком помещаться в овал:
  - **Макушка**: `y ≈ -130` (в `2x`-кадре `2048` это `y ≈ 436`) — верх головы/причёски выступает НАД кольцом.
  - **Подбородок**: `y ≈ 410` (в `2x`-кадре `y ≈ 711`), глаза на `y ≈ 150–200`, лицо по центру `x ≈ 512`.
- **Талия / Пояс (линия талии или ремня)**: `y ≈ 715` (в `2x`-кадре `y ≈ 1230`), а нижний срез торса/юбки доходит до нижней внутренней кромки кольца (`y ≈ 865`).
- **Маска и детали поверх обода**:
  - Розовая маска (`target: "canvas"`) покрывает весь холст `3x` (`-1024..2048`).
  - Нажатие `preset1` (`tokenatra_apply_preset({ session_id, preset: "preset1" })`) оставляет всё внутри кольца и верхний поп-аут (голову/шляпу), срезая низ по внутренней кромке кольца.
  - Кисти рук, эфесы оружия или щиты по бокам, которые должны лежать ПОВЕРХ обода кольца, восстанавливаются мягкой кистью (`op: "restore"` с `circle` и `feather: 8`) в области `y ≈ 550–700`.
- **Экспорт**:
  - `scale_mode: "auto"` (или `"2"`): так как макушка/шляпа выступает выше кольца (`y < 0`), авто-масштаб сохраняет кадр `2x` (`2048×2048` при `size: 1024`), чтобы голова и шляпа не обрезались краем файла.
  - Для `subject.webp` (вырезка без кольца с тенью): включить тень (`angle: 135, distance: 7, blur: 3, opacity: 0.4`) и экспортировать с `with_ring: false, scale_mode: "auto"`.

## Модель (remove_bg)

`models/*.onnx`, выбор в Настройках. RMBG-2.0/BiRefNet — ImageNet-нормализация;
BEN2 — plain ToTensor + gentle-рефайн (см. `infer.rs`, флаг `gentle` по имени файла).

## Foundry-модуль

`foundry-module/tokenatra-connector/` — коннектор; сборка zip: `build_module.bat`
(использует `build_studio.py` — единственный оставшийся Python-файл, stdlib-only,
нужен только при сборке модуля).

## Gotchas

- Портрет с прозрачным фоном: `/remove_bg` НЕ вызывать (модель перематтит и
  съест края). Проверка: доля непрозрачных пикселей < 92%.
- `scale_mode: "auto"` при крупном субъекте даст авто-кроп 2×/3× и «медальон»
  вместо кольца на весь канвас. Для стиля серии — всегда `"1"`.
- eraser-маски оперируют в координатах канваса ДО transform (маска тянется
  вместе со слоем субъекта).
- У всех колец каталога тёмный диск-подложка; опознавать кольцо по альфе нельзя
  (силуэт одинаков) — только по рисунку обода.
- Detached-блобы (< 0.02% канваса) в subject — мусор вырезки, игнорировать.

## История

Python-версия (Flask + pywebview) удалена в коммите `baf0b68` — полный порт
в Rust: `52a3eda` и далее. Референс удалённой семантики: `git show baf0b68^:agent_pipeline.py`.
