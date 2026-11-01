# Agent API — редактор токенов для ИИ-агентов

Tokenatra отдаёт headless-редактор токенов через три входа поверх одного ядра:
HTTP API, MCP-сервер и OpenAI-совместимые схемы. Агент (Claude, GLM, DeepSeek,
любая другая модель) может без человека пройти полный цикл: источник → удаление
фона → правка маски → кольцо → эффекты → экспорт.

**Модель доверия — как у Blender MCP**: без ключей, без тумблеров, always-on,
пока запущено приложение (или `python server.py`). Доверенная зона —
localhost-процессы того же пользователя.

---

## Инвариант безопасности

> В `/agent/v1/*` никогда не попадает операция, которая (а) исполняет код,
> (б) читает секреты/конфиг, (в) пишет на диск за пределами явно разрешённых
> папок, (г) ходит в сеть от имени пользователя.

Если такая операция когда-нибудь понадобится — она получает **собственный**
ключ/тумблер, а не весь API. Текущие гарантии:

- источники сессий — только `base64`/`data_url` (никаких путей на диске —
  закрытый на аудите вектор «чтение файлов чужой учёткой через loopback»);
- ядро не пишет на диск и не ходит в сеть; экспорт отдаёт байты в ответе;
- `/agent/*` недоступен: чужим сайтам (Origin/Sec-Fetch-Site), LAN-клиентам,
  префлайтам; CORS-заголовки на агентные роуты не ставятся никогда;
- доступ: `local_tool` (без Origin и Sec-Fetch-Site) или same-origin браузер,
  **всегда с заголовком-маркером** `X-Tokenatra-Agent: 1`.

Принятые риски (зафиксировано по итогам двух ревью): другой локальный
пользователь той же машины может пользоваться API, пока приложение запущено;
GPU-инференс доступен локальным процессам без аутентификации — это не хуже
существующей поверхности (`/process` при `allow_browser=True` открыт любому
сайту и читаем через CORS).

## Три входа

| Кому | Вход | Как |
|---|---|---|
| Claude Desktop / Code, OpenCode (GLM, DeepSeek), Cursor | **MCP stdio** | `TokenatraMCP.exe` (сборка) или `python tokenatra_mcp.py` |
| DeepSeek raw API, любые OpenAI-совместимые скрипты | **HTTP + tools_schema** | `GET /agent/v1/tools_schema` → `tools=[...]` в chat.completions |
| человек, агент в репозитории | **HTTP + curl** | `curl -H "X-Tokenatra-Agent: 1" http://127.0.0.1:7878/agent/v1/manifest` |

Приложение должно быть запущено (окно, трей или `python server.py`).
Адрес для MCP-прокси переопределяется env `TOKENATRA_URL`.

## Конфигурации MCP

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "tokenatra": {
      "command": "C:\\Program Files\\Tokenatra\\TokenatraMCP\\TokenatraMCP.exe"
    }
  }
}
```

OpenCode (`opencode.json`; сюда же GLM Coding Plan и DeepSeek):

```json
{
  "mcp": {
    "tokenatra": {
      "type": "local",
      "command": ["C:\\Program Files\\Tokenatra\\TokenatraMCP\\TokenatraMCP.exe"],
      "enabled": true
    }
  }
}
```

Из исходников: `"command": ["python", "Z:\\Tokenatra\\tokenatra_mcp.py"]`.
См. также `examples/deepseek_demo.py` — цикл function-calling для raw API.

## HTTP API (`/agent/v1/*`)

Все запросы — с заголовком `X-Tokenatra-Agent: 1`. Тело — JSON. Координаты —
логический канвас **1024×1024** (0,0 — левый верхний угол). Ответы-ошибки
машиночитаемы: `{"error", "code", "hint"?, "valid"?}`; каталог кодов в
`agent_api.py`.

```
GET  /manifest                 лимиты, дефолты, список операций
GET  /assets                   кольца + пресеты (name, builtin)
GET  /tools_schema             OpenAI function-calling схемы

POST /sessions                 {image: base64|data_url} -> session_id
GET  /sessions/<id>            состояние + recipe (append-only лог)
DEL  /sessions/<id>            закрыть

POST /sessions/<id>/remove_bg  {edge_blur?}            (ONNX-инференс)
POST /sessions/<id>/mask       {op, shapes?/preset?/px?, protect?}
POST /sessions/<id>/transform  {scale?, rotation?, offset_x?, offset_y?}
POST /sessions/<id>/ring       {name|null}
POST /sessions/<id>/effects    {shadow?{...}, color?{...}}
GET  /sessions/<id>/inspect    метрики (+?include_image=1)
GET  /sessions/<id>/preview    ?size=384 -> webp
POST /sessions/<id>/export     {format, quality?, size?, scale_mode?, with_ring?} -> байты
```

`mask.op`: `erase|restore|apply_preset|invert|feather|grow`;
`shapes`: `[{"circle":{cx,cy,r}}, {"rect":{x0,y0,x1,y1}}, {"stroke":{points,Width}}]`.
`erase` уважает маску защиты (`mask.png` / сайдкар кольца, семантика фронтенда).
`transform.scale` — относительно вписанного в канвас размера (1.0 = fit).
`export.scale_mode`: `auto|1|2|3`, итоговые пиксели = `size × scale`.

Лимиты: 4 сессии (LRU), TTL 30 мин (скользящий — сбрасывается любой
операцией), 200 операций/сессию (при исчерпании — 429 с recipe в теле),
тело ≤ 15 МБ, исходник ≤ 2048 px по стороне.

## Inspect — обратная связь

```json
{
  "coverage_pct": 26.9, "bbox": [256, 120, 768, 854],
  "grid_3x3_pct": [[...]], "edge_px": 4210, "alpha_hist": [...],
  "warnings": ["mask is almost empty: nothing to export"],
  "delta": {"coverage_dpct": -4.1}
}
```
Текстовым моделям достаточно чисел; мультимодальным добавьте
`?include_image=1` (превью webp ≤384px). Рекомендуемый цикл: правка →
`inspect` → при необходимости уточнение → `export`.

## Recipe

Каждая сессия накапливает append-only лог операций (`GET /sessions/<id>` →
`recipe`). При истечении TTL или исчерпании лимита ошибочный ответ содержит
recipe — агент может пересоздать сессию и повторить цепочку. Идемпотентный
replay отложен до реального юзкейса (ревью: детерминизм зависит от версий
Pillow/модели).

## Паритет с UI и известные расхождения

Композиция — порт `TokenCanvas.renderForSave`: рендер `size×3` → кольцо снизу
→ субъект (fit·scale, поворот, смещение) → маска `destination-in` → тень от
замаскированного субъекта → кроп `size×usedScale` (`auto` — порт
`detectUsedScale`). Расхождения:

- `transform.scale` относительный (fit=1.0), во фронтенде — абсолютный;
- тень/цветокоррекция визуально эквивалентны, не бит-в-бит
  (canvas `blur(R)` ≈ `GaussianBlur(R/2)`; `ImageEnhance` vs `ctx.filter`);
- база капится до 2048 px (UI — до 6144): агентный путь чуть грубее на ×3;
- регрессии ядра ловятся тестами (`tests/test_agent_pipeline.py`); сверка с
  GUI — разовый ритуал из 5–7 сценариев при изменениях композиции.

## Сборка

`TokenatraMCP.exe` — второй entry в `build.spec` (консольный, без GUI-стекa).
macOS/Linux: `python tokenatra_mcp.py` системным Python (прокси — stdlib-only).

## Файлы

- `agent_pipeline.py` — ядро (Pillow/numpy, без Flask)
- `agent_api.py` — HTTP-транспорт (blueprint)
- `tokenatra_mcp.py` — MCP stdio-прокси (stdlib-only)
- `examples/deepseek_demo.py` — демо raw-API агента
- тесты: `tests/test_agent_guard.py`, `tests/test_agent_pipeline.py`,
  `tests/test_agent_api.py`, `tests/test_mcp_stdio.py`
