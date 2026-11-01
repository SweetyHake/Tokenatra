#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeepSeek-демо: raw-API агент для Tokenatra (без MCP).

Любая модель с OpenAI-совместимым function calling (DeepSeek, GLM, GPT…)
проходит полный цикл: источник -> remove_bg -> маска -> кольцо -> экспорт.

Зависимости: pip install openai
Запуск:
    set DEEPSEEK_API_KEY=sk-...
    set TOKENATRA_URL=http://127.0.0.1:7878      # опционально
    python examples/deepseek_demo.py путь/к/картинке.png " prompt: гоблин с топором, кольцо красное"

Схемы тулзов подтягиваются с живого сервера (GET /agent/v1/tools_schema),
поэтому демо и MCP никогда не расходятся.
"""
import base64
import json
import os
import sys
import urllib.request

BASE_URL = os.environ.get('TOKENATRA_URL', 'http://127.0.0.1:7878')
MARKER = {'X-Tokenatra-Agent': '1', 'Content-Type': 'application/json'}
MODEL = os.environ.get('DEMO_MODEL', 'deepseek-chat')
MAX_STEPS = 24

SYSTEM_PROMPT = """You operate the Tokenatra token editor via function calls.
Workflow: create a session from the provided image (read the file, pass base64),
remove the background, inspect, adjust mask/transform/effects/ring as the user
asks, inspect again, then export. Text-only feedback: rely on inspect metrics
(coverage_pct, bbox, grid_3x3_pct, warnings). When the user's request is
fulfilled, answer with a short summary and stop calling tools."""


def http(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE_URL + path, data=data, headers=MARKER,
                                 method=method)
    with urllib.request.urlopen(req, timeout=180) as r:
        raw = r.read()
        if r.headers.get('Content-Type', '').startswith('application/json'):
            return json.loads(raw)
        return {'_bytes': len(raw), '_mime': r.headers.get('Content-Type')}


def execute_tool(name, args):
    """Маппинг OpenAI-функций на HTTP-роуты (плоские параметры)."""
    sid = args.get('session_id')
    if name == 'tokenatra_manifest':
        return http('GET', '/agent/v1/manifest')
    if name == 'tokenatra_list_assets':
        return http('GET', '/agent/v1/assets')
    if name == 'tokenatra_create_session':
        return http('POST', '/agent/v1/sessions', {'image': args['image_b64']})
    if name == 'tokenatra_remove_background':
        return http('POST', f'/agent/v1/sessions/{sid}/remove_bg',
                    {'edge_blur': args.get('edge_blur', 1.0)})
    if name == 'tokenatra_edit_mask':
        body = {k: args[k] for k in ('op', 'preset', 'px', 'protect')
                if k in args}
        if args.get('shapes_json'):
            body['shapes'] = json.loads(args['shapes_json'])
        return http('POST', f'/agent/v1/sessions/{sid}/mask', body)
    if name == 'tokenatra_apply_preset':
        return http('POST', f'/agent/v1/sessions/{sid}/mask',
                    {'op': 'apply_preset', 'preset': args['preset']})
    if name == 'tokenatra_set_transform':
        body = {k: args[k] for k in ('scale', 'rotation', 'offset_x', 'offset_y')
                if k in args}
        return http('POST', f'/agent/v1/sessions/{sid}/transform', body)
    if name == 'tokenatra_set_ring':
        return http('POST', f'/agent/v1/sessions/{sid}/ring',
                    {'name': args.get('name')})
    if name == 'tokenatra_set_effects':
        body = {}
        if args.get('shadow_json'):
            body['shadow'] = json.loads(args['shadow_json'])
        if args.get('color_json'):
            body['color'] = json.loads(args['color_json'])
        return http('POST', f'/agent/v1/sessions/{sid}/effects', body)
    if name == 'tokenatra_inspect':
        qs = '?include_image=1' if args.get('include_image') else ''
        return http('GET', f'/agent/v1/sessions/{sid}/inspect{qs}')
    if name == 'tokenatra_preview':
        return http('GET', f"/agent/v1/sessions/{sid}/preview?size="
                           f"{int(args.get('size', 384))}")
    if name == 'tokenatra_export':
        body = {k: args[k] for k in ('format', 'quality', 'size', 'scale_mode',
                                     'with_ring') if k in args}
        return http('POST', f'/agent/v1/sessions/{sid}/export', body)
    if name == 'tokenatra_close_session':
        return http('DELETE', f'/agent/v1/sessions/{sid}')
    return {'error': f'unknown tool {name}'}


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    image_path = sys.argv[1]
    user_request = ' '.join(sys.argv[2:]) or 'make a clean token with a ring'

    try:
        from openai import OpenAI
    except ImportError:
        sys.exit('pip install openai')

    key = os.environ.get('DEEPSEEK_API_KEY')
    if not key:
        sys.exit('set DEEPSEEK_API_KEY first')
    client = OpenAI(api_key=key, base_url='https://api.deepseek.com')

    with open(image_path, 'rb') as f:
        image_b64 = 'data:image/png;base64,' + base64.b64encode(f.read()).decode()

    tools = http('GET', '/agent/v1/tools_schema')
    print(f'{len(tools)} tools loaded from {BASE_URL}')

    messages = [
        {'role': 'system', 'content': SYSTEM_PROMPT},
        {'role': 'user', 'content': f'Image (base64 data URL):\n{image_b64}\n\n'
                                    f'Request: {user_request}'},
    ]

    for step in range(MAX_STEPS):
        resp = client.chat.completions.create(
            model=MODEL, messages=messages, tools=tools)
        msg = resp.choices[0].message
        messages.append(msg.model_dump())

        if not msg.tool_calls:
            print('DONE:', msg.content)
            return

        for call in msg.tool_calls:
            args = json.loads(call.function.arguments or '{}')
            print(f'-> {call.function.name}({json.dumps(args)[:120]})')
            try:
                result = execute_tool(call.function.name, args)
            except urllib.error.HTTPError as e:
                result = json.loads(e.read().decode('utf-8', 'replace'))
            except Exception as e:  # приложение не запущено и т.п.
                result = {'error': str(e), 'code': 'app_down'}
            print(f'<- {json.dumps(result)[:160]}')
            messages.append({
                'role': 'tool',
                'tool_call_id': call.id,
                'content': json.dumps(result)[:8000],
            })

    print('step limit reached')


if __name__ == '__main__':
    main()
