# Tokenatra — AGENTS.md (Rust edition)

Инструкции для ИИ-агентов, работающих с этим репозиторием и приложением.

**Приложение полностью на Rust** (`rust/`): axum (HTTP) + ort (ONNX Runtime) +
wry/tao (GUI). Python-версии больше нет (удалена; см. историю git при необходимости).

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
POST /agent/v1/sessions                         {image: base64|data_url}
GET/DELETE /agent/v1/sessions/{id}
POST /agent/v1/sessions/{id}/remove_bg          {edge_blur}
POST /agent/v1/sessions/{id}/mask               {op, shapes|preset|px, protect}
      op: erase|restore (shapes rect/circle/stroke), apply_preset, invert, feather, grow
POST /agent/v1/sessions/{id}/transform          {scale, rotation, offset_x, offset_y}
POST /agent/v1/sessions/{id}/ring               {name|null}
POST /agent/v1/sessions/{id}/effects            {shadow, color}
GET  /agent/v1/sessions/{id}/preview?size=512
GET  /agent/v1/sessions/{id}/inspect?include_image=1
POST /agent/v1/sessions/{id}/export             {format, quality, size, scale_mode, with_ring}
```

Лимиты: 4 сессии (LRU), TTL 30 мин, 200 операций/сессию, body ≤ 15 МБ,
base dim ≤ 2048, экспорт 256–2048.

**Семантика transform**: логический канвас 1024, `L(f) = 512 + (f−512)·scale + offset`
(f — fit-координата источника). `scale_mode: "1"` — экспорт ровно область кольца 1×
(для стиля «кольцо на весь канвас + голова над кольцом» — ВСЕГДА "1", не "auto").

## Рецепты токенов и subject-файлов

Полное руководство: `W:\Images\STokens\TOKEN_STYLE.md` — композиция «кольцо 1× +
субъект», якоря (глаза 300–330, пояс 850–880), рецепты дуги, петля самопроверки,
реверс-инжиниринг. Анализ пакета: `W:\Images\STokens\SUBJECT_ANALYSIS.md`.

Кратко:
- **token = кольцо (1×) + субъект поверх**; голова выступает над кольцом, пояс у
  нижней кромки. Кольцо серии — `steel`. Никаких масок, если арт обрезан по пояс.
- **subject = портрет** (remove_bg если фон непрозрачный) **вписанный в квадрат,
  фигура ~52–60% высоты, дуга снизу** (circle ≈ 512, 350–460, r 260–370).

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
