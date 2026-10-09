/**
 * app.js — окно студии Tokenatra (ApplicationV2, синглтон).
 *
 * Особенности:
 *  - iframe грузится один раз; статус-бар обновляется императивно (DOM),
 *    без re-render — иначе ApplicationV2 пересоздаёт части и перезагружает
 *    iframe, теряя сессию редактирования;
 *  - исходящие postMessage буферизуются до tokenatra:ready и повторяются
 *    после каждой перезагрузки iframe (health-recovery);
 *  - подписка на window 'message' ставится в _onFirstRender и снимается
 *    в close — фильтры: event.source === iframe.contentWindow и точный
 *    origin Tokenatra.
 */

import * as api from './api.js';
import { relay } from './relay.js';
import { upload, sanitizeFileName, transliterate } from './upload.js';
import { applyExport, applyKindToActor, actorArtDataUrl } from './actions.js';

// Рендер из PARTS/шаблона даёт только HandlebarsApplicationMixin —
// голый ApplicationV2 требует собственные _renderHTML/_replaceHTML
const { ApplicationV2, HandlebarsApplicationMixin } = foundry.applications.api;

// Кеш собранной hosted-студии (текст ~0.5 МБ, не канвасы) — живёт до
// перезапуска страницы: повторное открытие окна мгновенно, без релея чанков.
// Живой UI из Tokenatra кешируется навсегда, статический фолбэк — на минуту:
// если ГМ запустил Tokenatra, следующее открытие/ретрай соберёт живой UI.
let _cachedStudioHtml = null;
let _cachedStudioIsLive = false;
let _cachedStudioAt = 0;
let _studioHtmlPromise = null;

const STUDIO_URL = '/modules/tokenatra-connector/studio/index.html';

/**
 * Собранная страница студии (живой UI из Tokenatra ГМа, фолбэк — статика
 * модуля). Дедупликация параллельных вызовов: прогрев и открытие окна
 * делят одну сборку.
 */
async function fetchStudioHtml() {
  const cacheFresh = Date.now() - _cachedStudioAt < 60000;
  if (_cachedStudioHtml && (_cachedStudioIsLive || cacheFresh)) return _cachedStudioHtml;
  if (!_studioHtmlPromise) {
    _studioHtmlPromise = buildStudioHtml().finally(() => { _studioHtmlPromise = null; });
  }
  return _studioHtmlPromise;
}

async function buildStudioHtml() {
  // 1) Живой UI из Tokenatra ГМа (чанки через сокет)
  let html = null;
  let live = false;
  try {
    const meta = await relay('studio_meta', {}, 5000);
    let buf = '';
    for (let i = 0; i < meta.chunks; i++) {
      const c = await relay('studio_chunk', { i }, 30000);
      buf += c.text;
    }
    html = buf;
    live = true;
  } catch (err) { /* ГМ офлайн — фолбэк */ }
  // 2) Статическая копия из модуля
  if (!html) {
    try {
      const res = await fetch(STUDIO_URL);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      html = await res.text();
    } catch (err) {
      return null;
    }
  }
  // srcdoc: относительный <base> браузеры применяют не всегда — заменяем
  // на абсолютный (origin мира) и переписываем относительные src/href
  const studioBase = window.location.origin + '/modules/tokenatra-connector/studio/';
  html = html.replace('<base href="/modules/tokenatra-connector/studio/">',
    `<base href="${studioBase}">` +
    `<script>window.__STUDIO_BASE__ = "${studioBase}";</script>`);
  html = html.replace(/(src|href)="\.\/([^"]*)"/g, (m, attr, rest) => `${attr}="${studioBase}${rest}"`);
  _cachedStudioHtml = html;
  _cachedStudioIsLive = !!live;
  _cachedStudioAt = Date.now();
  return html;
}

/** Прогрев студии в простое (вызывается на ready): открытие окна мгновенно. */
export function prefetchStudio() {
  fetchStudioHtml().catch(() => { /* тише: ретрай при открытии окна */ });
}

const MODULE_ID = 'tokenatra-connector';

export class TokenatraApp extends HandlebarsApplicationMixin(ApplicationV2) {

  static #instance = null;

  static DEFAULT_OPTIONS = {
    id: 'tokenatra-connector',
    classes: ['tkc-window'],
    window: {
      title: 'TKC.Title',
      icon: 'tkc-icon',
      resizable: true
    },
    position: { width: 1040, height: 780 },
    actions: {
      retry: TokenatraApp.#onRetry,
      closeWindow: TokenatraApp.#onCloseWindow
    }
  };

  static PARTS = {
    app: { template: 'modules/tokenatra-connector/templates/tokenatra-app.hbs' }
  };

  /**
   * Открыть студию. Механизм как в Tokenizer 2: синглтон по id приложения —
   * тот же актёр → bringToFront(), другой/без актёра → закрыть и открыть
   * заново (редактор всегда «в режиме редактирования» конкретного актёра).
   */
  static async open(actor = null) {
    // UI кешируется (текст), окно — нет: ✕ закрывает и освобождает память.
    // Если окно ещё открыто — просто поднимаем и (при акторе) грузим арт.
    const existing = foundry.applications.instances.get(TokenatraApp.DEFAULT_OPTIONS.id);
    if (existing instanceof TokenatraApp) {
      if (actor) existing.contextActor = actor;
      existing.bringToFront();
      if (actor) existing.#requestActorArt();
      // Каждое открытие — свежее зондирование Tokenatra (могла запуститься
      // или закрыться с прошлого раза); студия обновит conn на лету
      existing.#sendConn(true);
      return existing;
    }
    const app = new TokenatraApp();
    if (actor) app.contextActor = actor;
    await app.render(true);
    if (actor) app.#requestActorArt();
    // Свежее подключение + сообщаем студии режим (local: её /config ходит
    // в настоящий config.json на машине клиента). Студия сама стучится
    // (tokenatra:hello) — open() лишь ускоряет первый ответ
    app.#sendConn(true);
    return app;
  }

  #frame = null;
  #frameOrigin = null;
  #frameReady = false;
  #outbox = [];
  #messageHandler = null;
  #dragging = false;
  #connPromise = null;

  get contextActor() { return this.#contextActor; }
  set contextActor(actor) { this.#contextActor = actor ?? null; }
  #contextActor = null;

  async _prepareContext(options) {
    // Hosted-студия раздаётся модулем — iframe доступен всегда; Tokenatra
    // нужна только для AI (отсутствие покажут ошибки релея при нажатии)
    return { connected: true, isOld: false, isAuth: false, oldVersion: null };
  }

  _onFirstRender(context, options) {
    super._onFirstRender(context, options);
    this.#messageHandler = (event) => this.#onWindowMessage(event);
    window.addEventListener('message', this.#messageHandler);
    // Опрос /health не нужен: hosted-студия раздаётся модулем и не зависит
    // от жизни Tokenatra (её недоступность видна по ошибкам релея)
  }

  _onRender(context, options) {
    super._onRender(context, options);
    if (options.parts && !options.parts.includes('app')) return;
    this.#frame = this.element.querySelector('.tkc-frame');
    if (this.#frame) {
      // Foundry отдаёт .html из модулей как text/plain (анти-XSS) — берём
      // текст и отдаём в iframe через srcdoc (same-origin, рендерится)
      this.#loadStudio();
      // srcdoc наследует origin родителя — postMessage адресуем в мир
      this.#frameOrigin = window.location.origin;
      // После (re)load страницы iframe досылаем контекстный арт. Готовность
      // моста не сбрасываем: tokenatra:ready приходит раньше load, а сброс
      // здесь заморозил бы outbox навсегда (flush по ready уже прошёл)
      this.#frame.addEventListener('load', () => {
        if (this.#contextActor) this.#requestActorArt();
      });
    } else {
      this.#frameReady = false;
    }
  }

  async close(options = {}) {
    if (this.#messageHandler) {
      window.removeEventListener('message', this.#messageHandler);
      this.#messageHandler = null;
    }
    return super.close(options);
  }

  // ===== Взаимодействие с iframe =====

  /**
   * Загрузка студии. Приоритет — ЖИВОЙ UI из Tokenatra ГМа: клиент ГМа
   * собирает самодостаточную страницу со своего localhost (инлайн CSS/JS +
   * транспорт) и отдаёт чанками через сокет мира. Так игроки всегда
   * получают интерфейс актуальной версии. ГМ офлайн — офлайн-фолбэк:
   * статическая копия из модуля (собирается build_studio.py).
   */
  async #loadStudio() {
    const frame = this.#frame;
    if (!frame) return;
    const html = await fetchStudioHtml();
    if (!html) {
      frame.srcdoc = `<pre style="color:#e93a3a;padding:16px">Tokenatra: студия недоступна</pre>`;
      return;
    }
    frame.srcdoc = html;
  }

  #postToFrame(msg) {
    const target = this.#frameOrigin;
    if (!this.#frame?.contentWindow || !target) return;
    this.#frame.contentWindow.postMessage(msg, target);
  }

  /**
   * Режим подключения для студии (tokenatra:conn). fresh — зондировать
   * заново, игнорируя кэш (каждое открытие окна). Промис кешируется:
   * hello из iframe и open() делят одно зондирование; результат 'none'
   * не кешируется — следующее открытие проверит доступность снова.
   */
  #sendConn(fresh = false) {
    if (fresh) this.#connPromise = null;
    if (!this.#connPromise) {
      this.#connPromise = api.ensureConnection({ fresh })
        .then(({ mode, base }) => ({ type: 'tokenatra:conn', mode, base }))
        .catch(() => ({ type: 'tokenatra:conn', mode: 'none' }));
    }
    this.#connPromise.then(msg => {
      if (msg.mode === 'none') this.#connPromise = null;
      this.#postToFrame(msg);
      // Значок окна (красный, если Tokenatra не запущена) + версия в тайтлбар
      this.#applyConnUi(msg.mode);
      if (msg.mode !== 'none') {
        this.#fetchVersion()
          .then(ver => this.#applyConnUi('connected', ver))
          .catch(() => {});
      }
    });
  }

  /** Состояние в хроме окна: красный значок офлайн, версия Tokenatra в титуле. */
  #applyConnUi(mode, version = null) {
    const el = this.element;
    if (el) el.classList.toggle('tkc-offline', mode === 'none');
    const title = this.window?.title;
    if (title) {
      const base = game.i18n.localize('TKC.Title');
      title.textContent = version ? `${base} ${version}` : base;
    }
  }

  /** Версия запущенной Tokenatra: напрямую в local, через ГМа в host. */
  async #fetchVersion() {
    try {
      const { mode, base } = await api.ensureConnection();
      if (mode === 'local') {
        const r = await fetch(base + '/version');
        if (!r.ok) return null;
        return (await r.json())?.version || null;
      }
      if (mode === 'host') {
        const v = await relay('version', {}, 8000);
        return v?.version || null;
      }
    } catch (err) { /* версии нет — титул без номера */ }
    return null;
  }

  #requestActorArt() {
    const actor = this.#contextActor;
    if (!actor) return;
    // Флаги от прошлых сохранений: вместо текущего арта актора грузим
    // оригинал (исходник до AI) и применяем снапшот настроек в студии
    this.#buildRestoreMsg(actor)
      .catch(() => null)
      .then(restoreMsg => {
        return restoreMsg || actorArtDataUrl(actor).then(dataUrl => ({
          type: 'tokenatra:load',
          dataUrl,
          filename: `${sanitizeFileName(actor.name || 'actor')}.png`,
          sourcePath: actor.prototypeToken?.texture?.src || actor.img || null
        }));
      })
      .then(msg => {
        if (this.#frameReady) this.#postToFrame(msg);
        else this.#outbox.push(msg);
      })
      .catch(err => {
        console.warn(`Tokenatra: ${err.message}`);
      });
  }

  /** Сообщение восстановления из флагов (или null, если флагов нет). */
  async #buildRestoreMsg(actor) {
    const saved = actor.getFlag?.(MODULE_ID, 'tokenatra');
    if (!saved || !saved.original) return null;
    const res = await fetch(saved.original);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const blob = await res.blob();
    const dataUrl = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.onerror = () => reject(new Error('read failed'));
      reader.readAsDataURL(blob);
    });
    return {
      type: 'tokenatra:restore',
      originalUrl: dataUrl,
      filename: sanitizeFileName((saved.original.split('/').pop() || 'original.png')),
      sourcePath: saved.original,
      settings: saved.settings || null
    };
  }

  #onWindowMessage(event) {
    // Hosted-студия same-origin с миром — ждём сообщения только из неё
    if (event.origin !== window.location.origin) return;
    if (!this.#frame || event.source !== this.#frame.contentWindow) return;
    const data = event.data;
    if (!data || typeof data !== 'object') return;
    switch (data.type) {
      case 'tokenatra:ready':
        this.#frameReady = true;
        this.#flushOutbox();
        break;
      case 'tokenatra:hello':
        // Студия создалась позже, чем родитель успел отправить conn, —
        // отвечаем на её рукопожатие (иначе /config ждёт таймаут)
        this.#sendConn();
        break;
      case 'tokenatra:export':
        this.#handleExport(data);
        break;
      case 'tokenatra:error':
        console.error(`Tokenatra: ${data.message}`);
        break;
      case 'tokenatra:drag-start':
        this.#startWindowDrag(data.x, data.y);
        break;
      case 'tokenatra:window':
        if (data.action === 'close') this.close();
        break;
      case 'tokenatra:pick-file':
        this.#openFoundryPicker();
        break;
      case 'tokenatra:save':
        this.#handleFoundrySave(data);
        break;
      case 'tokenatra:relay': {
        // Умное подключение: локальная Tokenatra исполняет напрямую,
        // иначе релей через клиент ГМа (только для сокетного режима важен таймаут)
        api.ensureConnection()
          .then(({ mode }) => {
            const timeout = data.action === 'process' ? 180000 : 20000;
            return mode === 'local'
              ? api.executeLocally(data.action, data.payload ?? {})
              : relay(data.action, data.payload ?? {}, timeout);
          })
          .then(result => this.#postToFrame({
            type: 'tokenatra:relay-result', relayId: data.relayId, ok: true, result
          }))
          .catch(err => this.#postToFrame({
            type: 'tokenatra:relay-result', relayId: data.relayId, ok: false, error: err.message
          }));
        break;
      }
    }
  }

  /** «Открыть» в студии — внутренний браузер файлов Foundry. */
  #openFoundryPicker() {
    const target = String(game.settings.get(MODULE_ID, 'uploadTarget') || 'tokenatra/');
    const picker = new FilePicker({
      type: 'image',
      current: target,
      callback: async (path) => {
        try {
          const res = await fetch(path);
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          const blob = await res.blob();
          const dataUrl = await new Promise((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result);
            reader.onerror = () => reject(new Error('read failed'));
            reader.readAsDataURL(blob);
          });
          this.#postToFrame({
            type: 'tokenatra:load',
            dataUrl,
            filename: String(path).split('/').pop() || 'image.png',
            sourcePath: String(path)
          });
        } catch (err) {
          console.error(`Tokenatra: ${err.message}`);
        }
      }
    });
    picker.render(true);
  }

  /**
   * Имя файла от контекстного актёра: транслитерация любого алфавита в
   * латиницу. Если имя выродилось (нет актёра или алфавит непереводим) —
   * null, вызывающий падает на имя из студии.
   */
  #actorFileBase(suffix = '') {
    const name = this.#contextActor?.name;
    if (!name) return null;
    const base = sanitizeFileName(transliterate(name).replace(/[^a-z0-9.\- ]/gi, ''));
    return base ? `${base}${suffix}` : null;
  }

  /**
   * Сохранение из студии: токен/субъект/портрет → Data/<uploadTarget>,
   * затем применение к контекстному актёру (или диалог создания).
   */
  async #handleFoundrySave(data) {
    const kind = data.kind === 'subject' ? 'subject'
      : data.kind === 'portrait' ? 'portrait' : 'token';
    try {
      console.info(game.i18n.localize('TKC.InfoReceiving'));
      const blob = await api.fetchExport(data.url);
      const ext = data.format || 'webp';
      const actorBase = this.#actorFileBase(`-${kind}`);
      const fileName = actorBase
        ? `${actorBase}.${ext}`
        : sanitizeFileName(transliterate(data.filename || `save.${ext}`));
      const path = await upload(blob, fileName);
      if (!path) return; // прав на загрузку нет — файл скачан локально
      const metrics = {
        scale: Number(data.scale) || 1,
        scaleX: Number(data.scaleX) || 1,
        scaleY: Number(data.scaleY) || 1
      };
      const actor = this.#contextActor;
      if (actor && actor.isOwner) {
        await applyKindToActor(actor, kind, path, metrics);
        console.info(game.i18n.format('TKC.DoneSaved', { name: actor.name }));
        // Снапшот токена во флаги актора: оригинал отдельным файлом,
        // в флаге — путь к нему и все применённые настройки вида
        if (data.tokenatra || data.original) {
          await this.#saveTokenatraFlags(actor, data, actorBase, path);
        }
      } else {
        await applyExport(path, fileName, { contextActor: null, autoApply: false, kind, metrics });
      }
    } catch (err) {
      console.error(err.message || game.i18n.localize('TKC.ErrorApply'));
    }
  }

  /**
   * Флаги актора для продолжения редактирования. Оригинал НЕ дублируется:
   * - изображение уже лежит во Foundry (загружено из арта актора, выбора
   *   файла или прошлого оригинала) — в флаг пишется существующий путь;
   * - тот же оригинал, что при прошлом сохранении (совпал originalHash) —
   *   переиспользуется прежний файл;
   * - иначе — загрузка общего <имя>-original.png (у Token и Subject он один).
   * Ошибка флагов не роняет само сохранение.
   */
  async #saveTokenatraFlags(actor, data, actorBase, tokenPath) {
    try {
      const existing = actor.getFlag?.(MODULE_ID, 'tokenatra');
      let originalPath = null;
      let originalHash = null;
      if (data.originalFoundryPath) {
        // Изображение уже хранится во Foundry — новой копии не создаём
        originalPath = data.originalFoundryPath;
      } else if (data.original) {
        originalHash = data.originalHash || null;
        if (originalHash && existing?.original && existing.originalHash === originalHash) {
          originalPath = existing.original;
        } else {
          const origBlob = await api.fetchExport(data.original);
          // Оригинал у Token и Subject один и тот же — общий файл без суффикса
          // вида, перезаписывается при следующем сохранении с новым оригиналом
          const origName = `${this.#actorFileBase() || sanitizeFileName(transliterate(this.#contextActor?.name || 'token'))}-original.png`;
          originalPath = await upload(origBlob, origName);
        }
      }
      await actor.setFlag(MODULE_ID, 'tokenatra', {
        v: 1,
        savedAt: data.tokenatra?.savedAt || new Date().toISOString(),
        token: tokenPath,
        original: originalPath,
        originalHash,
        settings: data.tokenatra || null
      });
    } catch (err) {
      console.warn(`Tokenatra: flag save failed: ${err?.message || err}`);
    }
  }

  /**
   * Перетаскивание окна за тайтлбар Tokenatra (шапка Foundry скрыта).
   * На время драга глушим pointer-events у iframe — иначе мышь «проваливается»
   * в iframe и события обрываются на его границе. Позиция считается АБСОЛЮТНО
   * (курсор минус захват с точки нажатия) — без накопления дрейфа от клампа
   * setPosition, из-за которого окно дёргалось и «отставало».
   */
  #startWindowDrag(startX, startY) {
    if (!this.#frame || this.#dragging) return;
    if (!Number.isFinite(startX)) startX = window.innerWidth / 2;
    if (!Number.isFinite(startY)) startY = 20;
    this.#dragging = true;
    const frame = this.#frame;
    frame.style.pointerEvents = 'none';
    document.body.classList.add('tkc-dragging');
    const offsetX = startX - this.position.left;
    const offsetY = startY - this.position.top;
    const onMove = (ev) => {
      this.setPosition({ left: ev.clientX - offsetX, top: ev.clientY - offsetY });
    };
    const endDrag = () => {
      if (!this.#dragging) return;
      this.#dragging = false;
      frame.style.pointerEvents = '';
      document.body.classList.remove('tkc-dragging');
      window.removeEventListener('mousemove', onMove, true);
      window.removeEventListener('mouseup', endDrag, true);
      window.removeEventListener('blur', endDrag, true);
    };
    // capture-фаза: события не перехватываются UI Foundry; blur — если курсор
    // ушёл за пределы браузера с зажатой кнопкой, драг не «залипает»
    window.addEventListener('mousemove', onMove, true);
    window.addEventListener('mouseup', endDrag, true);
    window.addEventListener('blur', endDrag, true);
    onMove({ clientX: startX, clientY: startY });
  }

  #flushOutbox() {
    const queue = this.#outbox.splice(0);
    queue.forEach(msg => this.#postToFrame(msg));
  }

  async #handleExport(data) {
    try {
      console.info(game.i18n.localize('TKC.InfoReceiving'));
      const blob = await api.fetchExport(data.url);
      const ext = data.format || 'webp';
      const actorBase = this.#actorFileBase();
      const fileName = actorBase
        ? `${actorBase}.${ext}`
        : sanitizeFileName(data.filename || `token.${ext}`);
      const path = await upload(blob, fileName);
      if (!path) return; // прав нет: файл скачан локально, предупреждение показано
      const autoApply = game.settings.get(MODULE_ID, 'autoApply');
      await applyExport(path, fileName, {
        contextActor: this.#contextActor,
        autoApply
      });
    } catch (err) {
      console.error(err.message || game.i18n.localize('TKC.ErrorApply'));
    }
  }

  // ===== Статус =====


  // ===== Кнопки (actions; this = приложение) =====

  static async #onRetry() {
    await this.render();
  }

  static async #onCloseWindow() {
    await this.close(); // настоящее закрытие: память освобождается
  }
}
