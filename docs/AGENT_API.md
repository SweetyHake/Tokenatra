# Agent API — редактор токенов для ИИ-агентов

Tokenatra отдаёт headless-редактор токенов через три входа поверх одного Rust-ядра (`axum` + `ort` + `image`):
HTTP API (`/agent/v1/*`), нативный MCP-сервер (`TokenatraMCP.exe`) и OpenAI-совместимые схемы (`/agent/v1/tools_schema`). Агент может полностью автоматически пройти весь цикл: загрузка арта по локальному пути или base64 → умное удаление фона → анатомическое кадрирование → пресет и правка маски → кольцо → эффекты → аудит рентгеном и авто-линтером → экспорт портрета, токена, subject и контакт-листа.

**Модель доверия — как у Blender MCP**: без API-ключей, always-on, пока запущена Tokenatra (окно, трей или `--server`). Доверенная зона — localhost-процессы того же пользователя.

---

## Инвариант безопасности

- Взаимодействие разрешено только с `127.0.0.1` / `localhost` / `::1`;
- `/agent/v1/*` блокирует кросс-ориджин браузерные запросы (`Origin` / `Sec-Fetch-Site`), LAN-клиентов и preflight CORS;
- каждый запрос обязан нести заголовок-маркер `X-Tokenatra-Agent: 1` (подробности в `rust/src/http/mod.rs` -> `guard_mw`).

---

## Три входа

| Кому | Вход | Как |
|---|---|---|
| Google Antigravity, OpenCode, Claude Desktop / Code, Cursor, Windsurf | **MCP stdio** | Нативный бинарник `TokenatraMCP.exe` |
| Raw API и скрипты с function calling | **HTTP + `tools_schema`** | `GET /agent/v1/tools_schema` → `tools=[...]` |
| Прямые HTTP-вызовы и отладка | **HTTP** | `curl -H "X-Tokenatra-Agent: 1" http://127.0.0.1:7878/agent/v1/manifest` |

`TokenatraMCP.exe` сам находит и при необходимости поднимает сервер `tokenatra.exe --server`. Адрес можно переопределить через переменную окружения `TOKENATRA_URL`, путь к бинарнику — через `TOKENATRA_EXE`.

---

## Регистрация MCP и защита от даунгрейда

Для регистрации во всех обнаруженных клиентах (`~/.gemini/config/mcp_config.json`, `~/.config/opencode/opencode.json`, Cursor, Windsurf, Claude Desktop) используется команда:

```powershell
TokenatraMCP.exe --install-mcp
```

При обычном запуске приложения вызов `register_mcp_clients(false)` прощупывает уже прописанный в конфиге бинарник через однострочный JSON-RPC `initialize` и:
- **сохраняет** существующую запись, если в ней указана более новая версия;
- **сохраняет** существующую запись при равной версии, если там прописана активная dev-сборка (`target\release\TokenatraMCP.exe`), чтобы установленный пакет не перетирал локальную сборку разработчика;
- опция `--install-mcp` (`force=true`) принудительно обновляет конфиги и создаёт основной `~/.gemini/config/mcp_config.json`, если его ещё не было.

Оба бинарника поддерживают `--version` и `-V` (`tokenatra <ver>`, `tokenatra-mcp <ver>`).

---

## HTTP API (`/agent/v1/*`)

Все запросы — с заголовком `X-Tokenatra-Agent: 1`, тело — JSON (кроме `gui_mask`, который принимает `image/png` с альфой). Координаты `target: "canvas"` задаются в логическом пространстве `1x` кольца `1024×1024` (центр `(512, 512)`, область `2x` покрывает `[-512..1536]`).

```
GET  /manifest                         лимиты, дефолты, версия и список операций
GET  /assets                           доступные кольца и пресеты (builtin + пользовательские)
GET  /tools_schema                     OpenAI function-calling схемы

POST /pipeline                         полный цикл за один вызов (сессия → auto remove_bg → portrait → transform → preset → mask_ops → effects → export(s) → preview + qa)
POST /qa_sheet                         батчевый контакт-лист (`xray` 2×2, `eyes` 4×4, `showcase` 4×4) по `session_ids` или папкам (`subjects_dir` + `portraits_dir` + `transforms_json`, `input_dir`, `items`)
POST /gui_sync                         управление Live GUI Sync (`status`, `push`, `detach`)

POST /sessions                         {image_path | image: base64|data_url, ring?, live_sync?} -> session_id + inspect
GET  /sessions/<id>                    состояние + recipe
DEL  /sessions/<id>                    закрыть сессию

POST /sessions/<id>/remove_bg          {edge_blur?, showing_original?}
POST /sessions/<id>/mask               {target?: canvas|image|both, op, shapes?/preset?/px?, protect?}
POST /sessions/<id>/transform          {scale?, rotation?, offset_x?, offset_y?, fit_zone?}
POST /sessions/<id>/ring               {name|null}
POST /sessions/<id>/effects            {shadow?{...}, color?{...}}
POST /sessions/<id>/history            {action: undo|redo}
POST /sessions/<id>/portrait           {variant?, scale?, rotation?, offset_x?, offset_y?, size?, format?, quality?, output_path?}
POST /sessions/<id>/save_preset        {name}
GET  /sessions/<id>/preview            ?size=768&scale_mode=1|2|3|auto&with_ring=1&xray=1&grid=1&grid_step=20&crop=x0,y0,x1,y1&eyes=1&example=1&guides=1 -> webp
GET  /sessions/<id>/inspect            ?include_image=1&preview_size=768&xray=1&grid=1&grid_step=20&crop=...&eyes=1 -> метрики + mask.qa
POST /sessions/<id>/export             {format?, quality?, size?, scale_mode?, with_ring?, output_path?} -> JSON (если output_path) или байты

GET  /sessions/<id>/sync_state         снимок состояния и ревизий для Live GUI Sync
GET  /sessions/<id>/layer/<layer>      PNG-слои для студии (`original`, `cutout`, `canvas_mask`, `image_mask`)
POST /sessions/<id>/gui_update         обратная синхронизация трансформации/эффектов/кольца/переключателей из GUI в сессию
POST /sessions/<id>/gui_mask           обратная синхронизация PNG-маски кисти (`?target=canvas|image`) из GUI в сессию
```

### Лимиты

- Максимум **8 одновременных сессий** (LRU-вытеснение);
- Скользящий **TTL 30 минут**;
- До **200 операций** на сессию;
- Тело запроса ≤ **15 МБ**, максимальная сторона входного изображения в памяти ≤ **2048 px**;
- Экспорт токена/субъекта: **256–2048 px** (`scale_mode: auto|1|2|3`), портрет — до **4096 px**.

---

## Превью, макро-кроп, проверка взгляда и X-Ray (`preview`)

- **Дефолтный размер**: `size = 768` (`quality = 92` WebP; для `eyes=1` — `quality = 95`). При `xray=1`, `grid=1` или `example=1` по умолчанию включается `scale_mode = "2"`, что даёт рендер **`1536×1536`** — ровно сетку `2×2` нативных тайлов по `768 px` (Gemini `MEDIA_RESOLUTION_HIGH`) и под потолок `1568 px` у Claude.
- **Рентген и сетка**: на `xray=1` стёртые участки портрета отображаются полупрозрачным розовым призраком поверх бирюзовой шахматки, внутренний (`r_in = 343`) и внешний (`r_out = 468`) контуры кольца и линия горизонта `preset1` (`y = 564`) рисуются маркерами толщиной `2 px`, а на `grid=1` используется чёткий растровый шрифт `5×7`.
- **Снайперский макро-кроп (`crop`)**: принимает `crop: [x0, y0, x1, y1]` (или строку `"x0,y0,x1,y1"`) в логических `1x` координатах `target: "canvas"`, масштабирует выбранный участок до `1024 px` по длинной стороне и при `grid=1` автоматически уменьшает шаг сетки до `10 px` (если область `<= 520 px`).
- **Проверка взгляда (`eyes=1`)**: вырезает `1:1` кроп верхней области головы прямо из исходника (до сжатия/масштабирования токена) шириной около `640 px`.

---

## Inspect и математический авто-линтер (`mask.qa`)

Помимо статистики `bbox`, `coverage_pct`, `grid_3x3_pct` и `source_info` (`opaque_pct`, `already_transparent`, `fit1024_bbox`), `inspect` (а также `POST /pipeline`) прогоняет автоматический QA-линтер `mask.qa` по альфа-буферу сцен `3x` (`compute_mask_qa_from_s3`):

```json
{
  "clean": false,
  "detached_islands": [
    { "bbox": [96, 756, 144, 804], "center": [120, 780], "area_logical_px": 1728 }
  ],
  "bottom_ring_gap": false,
  "bottom_rim_fill_pct": 98.5,
  "step_564": [[120, 544, 166, 580]]
}
```

1. **`detached_islands`**: компоненты связности снаружи главного силуэта площадью ≥ `48` логических `px²` (отсечённая кисть на посохе, обломок клинка, мусорный фрагмент).
2. **`bottom_ring_gap` / `bottom_rim_fill_pct`**: проверка заполнения нижней дуги внутри `r_in` (`316..340` при `x = 412..612, y = 800..854`); если заполнение `< 85%`, выставляется флаг щели у нижнего обода.
3. **`step_564`**: горизонтальный обрыв альфы шириной ≥ `18` логических `px` вдоль линии горизонта `preset1` (`y = 564`) в боковых зонах `[-512..166]` и `[858..1536]`.

Все срабатывания дублируются в массиве `warnings` с точными координатами для немедленной правки через `mask` (`target: "canvas"`).

---

## Батчевые контакт-листы (`POST /qa_sheet` / `tokenatra_qa_sheet`)

- **`mode: "xray"`** (дефолт, по `4` на лист `2×2`): парные карточки (чистый токен с кольцом + рентген с розовым призраком и маркерами), тег вердикта авто-линтера (`OK` / `ISLAND` / `GAP` / `STEP`) в заголовке и метрика совмещения `err`.
- **`mode: "eyes"`** (по `16` на лист `4×4`): `1:1` кропы глаз исходников для массовой проверки направления взгляда.
- **`mode: "showcase"`** (по `16` на лист `4×4`): чистовая витрина токенов в кольце.
- Принимает как живые `session_ids`, так и файлы на диске (`subjects_dir` + опциональный `portraits_dir` и `transforms_json` с авто-выравниванием на Rust, `input_dir`, `image_paths`, `items`), поддерживает пагинацию `page`/`per_page`, `max_width` (по умолчанию `1536`) и `save_to`.

---

## Двусторонний Live GUI Sync (`POST /gui_sync` / `tokenatra_gui_sync`)

Позволяет показывать активную сессию агента прямо на холсте открытого окна студии и забирать обратно ручные правки человека (кисть/ластик, перетаскивание, зум, поворот, выбор кольца, эффекты, переключение «оригинал / без фона»):
- в состоянии `AgentSession` ведутся монотонные ревизии `rev`, `img_rev`, `canvas_mask_rev`, `image_mask_rev` и флаг `live_sync`;
- через `GuiCommand::EvalJs` (`rust/src/shell.rs`) бэкенд вызывает в открытом WebView `window.TokenAgentSync.onServerPush(sid, rev, force)`;
- `static/js/agentSync.js` проверяет, не редактирует ли пользователь собственную несинхронизированную картинку без `force=true` (при обычном `push` с `show_window` — отображает сессию, при открытии локального файла пользователем срабатывает `TokenAgentSync.detachLocal()`), пересчитывает координаты (`imageScale = scale * (1024/3) / maxDim`, `imageX = offset_x / 3`, `imageY = offset_y / 3`), а ручные правки пользователя возвращает на `/gui_mask` и `/gui_update`;
- в hosted-режиме для Foundry (`__STUDIO_BUILT__`, `__STUDIO_HOSTED__`, `state.embed`) синхронизация полностью отключена.

---

## Ключевые файлы реализации

- `rust/src/http/agent.rs` — ядро сессий, композиция, `mask.qa`, `qa_sheet`, `gui_sync` и роуты `/agent/v1/*`;
- `rust/src/http/mod.rs` — маршрутизация и `guard_mw`;
- `rust/src/mcp.rs` и `rust/src/bin/tokenatra_mcp.rs` — нативный MCP stdio-сервер и защита конфигов `register_mcp_clients`;
- `rust/src/shell.rs` и `rust/src/gui_control.rs` — мост `GuiCommand::EvalJs` в окно `wry`;
- `static/js/agentSync.js` — клиентский контроллер двусторонней синхронизации.
