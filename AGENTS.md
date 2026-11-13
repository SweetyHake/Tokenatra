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
  infer.rs           # remove_background (ort): BEN2
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

Лимиты: 8 сессий (LRU), TTL 30 мин, 200 операций/сессию, body ≤ 15 МБ,
base dim ≤ 2048, экспорт 256–2048.

**Семантика transform**: логический канвас 1024, `L(f) = 512 + (f−512)·scale + offset`
(f — fit-координата источника). `scale_mode: "1"` — экспорт ровно область кольца 1×
(для стиля «кольцо на весь канвас + голова над кольцом» — ВСЕГДА "1", не "auto").

## Рецепты токенов и subject-файлов (по шаблону `example.png`)

Полное руководство вшито в MCP (`tokenatra_recipe` и `<mcp_instructions>`).

1. **Направление взгляда и удаление фона**:
   - Поворот головы в 3/4 допустим, но **радужки и зрачки глаз обязаны смотреть строго в камеру** (проверять только по кропам глаз 1:1 в полном разрешении исходника).
   - `/remove_bg` вызывать только на непрозрачном исходнике (`already_transparent: false` или `opaque_pct >= 92`). Повторный вызов на прозрачном изображении съедает края и свечение.
2. **Анатомическое кадрирование (`transform`)**:
   - Масштаб и позиция (`scale`, `offset_x`, `offset_y`) определяются **строго по голове и поясу**, а не по габаритам оружия (не уменьшать персонажа ради поднятого меча или широкого арбалета).
   - В `1x` логических координатах кольца (`1024×1024`, центр `(512, 512)`):
     - **Макушка головы (без высоких шляп/рогов)**: `y ≈ -76` (выступает выше верхнего края `1x` кольца `y = 0`).
     - **Подбородок**: `y ≈ 200` (высота головы `≈ 276px`, `rx ≈ 112px`, центр лица `x ≈ 512`).
     - **Талия / Пояс**: `y ≈ 716`.
   - В `2x` превью (`1024×1024` при `scale_mode: "2"`, где `1x` кольцо занимает `[256..768]`):
     - **Овал головы (Макушка .. Подбородок)**: `y = 218..356` (`высота = 138px`, `rx = 56px`, `x = 512`).
     - **Талия / Пояс**: `y ≈ 615`.
3. **Точная геометрия кольца, защитной маски и `preset1` (в `1x` `1024×1024`, центр `(512, 512)`)**:
   - **Внутренний радиус кольца и защитной маски (`protect: true`)**: `r_in = 343` (низ `y = 855`; в `2x` превью `1024` `r_in = 171.5`).
   - **Внешний радиус стандартного кольца (`steel`)**: `r_out = 407` (низ `y = 919`; в `2x` превью `1024` `r_out = 203.5`).
   - **`preset1`**: оставляет `mask = 255` везде выше `y = 564`; ниже `y = 564` стирает всё снаружи `r = 342`.
4. **Координатные пространства и правила поп-аута (`target: "canvas"`)**:
   - На неквадратных исходниках (например `4:5`) не использовать `target: "image"` для точечных масок (независимое растяжение по X/Y смещает X-координаты). Все операции обрезки по кольцу и доводки оружия выполнять в `target: "canvas"` (`0..1024`) **строго после фиксации `transform`**.
   - **Устранение ступенек `preset1` на `y = 564`**:
     - Широкие плащи/шарфы выше `y = 564` (`y = 300..600`) подрезать по `r = 343` через `op: "erase", target: "canvas", protect: true`.
     - Выступающие боевые локти/наручи, чуть уходящие ниже `y = 564` (`y = 564..680`), плавно восстанавливать (`op: "restore", target: "canvas"`) по их нижнему анатомическому контуру.
   - **Строгие правила поп-аута**:
     - Никогда не выводить живот, пояс, ремни, сумки, юбки и висящие обереги ниже `r = 343` (`y = 855`).
     - В нижней полусфере (`y > 564`) снаружи `r = 343` могут находиться **только само оружие (клинок, гарда, рукоять, навершие, дуга/стремя арбалета) и сжимающая его кисть/предплечье**. Любую одежду/плащ/сумку в просвете между клинком и `r = 343` стирать (`op: "erase", protect: true`).
     - **Пустая опущенная рука всегда под маской**: свободная рука без оружия, опущенная вниз вдоль тела (`3–5` или `7–9` часов), обязана быть убрана внутрь `r = 343` (`op: "erase", protect: true`).
     - **Замкнутые контуры оружия**: пробивать прозрачность (`op: "erase", protect: false`) внутри петли стремени арбалетов и гарды-дужки сабель.
5. **Экспорт**:
   - Для `subject.webp` (вырезка без кольца с тенью): включить тень (`angle: 135, distance: 7, blur: 3, opacity: 0.4`) и экспортировать с `with_ring: false, scale_mode: "2", size: 1024`.

## Модель (remove_bg)

`models/*.onnx`, выбор в Настройках. По умолчанию используется `BEN2` (plain ToTensor + gentle-рефайн, см. `infer.rs`).

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
