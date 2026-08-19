const Converter = {
    results: new Map(),
    originalNames: new Map(),
    processingQueue: [],
    isProcessing: false,
    mediaExtensions: new Set([
        'mp3', 'wav', 'm4a', 'flac', 'aac', 'wma', 'opus', 'ogg',
        'mp4', 'mkv', 'mov', 'avi', 'webm', 'mpeg', 'mpg', 'm4v'
    ]),

    init() {
        this.setupFormatControls();
        this.setupDropzone();
        this.setupResultsControls();
    },

    setupFormatControls() {
        const formatSelect = $('convFormatSelect');
        if (formatSelect) {
            var saved = AppConfig.converter ? AppConfig.converter.format : 'webp';
            formatSelect.value = saved;
            state.converterFormat = saved;
            formatSelect.onchange = e => {
                state.converterFormat = e.target.value;
                if (AppConfig.setConverter) AppConfig.setConverter('format', e.target.value);
            };
        }

        const qualitySlider = $('convQualitySlider');
        const qualityValue = $('convQualityValue');
        if (qualitySlider) {
            var savedQ = AppConfig.converter ? AppConfig.converter.quality : 90;
            qualitySlider.value = savedQ;
            state.converterQuality = savedQ;
            if (qualityValue) qualityValue.textContent = savedQ + '%';
            qualitySlider.oninput = e => {
                state.converterQuality = parseInt(e.target.value);
                if (qualityValue) qualityValue.textContent = state.converterQuality + '%';
            };
            qualitySlider.onchange = e => {
                if (AppConfig.setConverter) AppConfig.setConverter('quality', parseInt(e.target.value));
            };
        }
    },

    setupDropzone() {
        const dropzone = $('convDropzone');
        const fileInput = $('convFileInput');
        if (!dropzone || !fileInput) return;

        dropzone.onclick = e => {
            e.stopPropagation();
            fileInput.click();
        };

        dropzone.ondragover = e => {
            e.preventDefault();
            dropzone.classList.add('dragover');
        };

        dropzone.ondragleave = () => dropzone.classList.remove('dragover');

        dropzone.ondrop = e => {
            e.preventDefault();
            dropzone.classList.remove('dragover');
            this.handleFiles(Array.from(e.dataTransfer.files));
        };

        fileInput.onchange = e => {
            this.handleFiles(Array.from(e.target.files));
            fileInput.value = '';
        };
    },

    setupResultsControls() {
        const clearAllBtn = $('convClearAllBtn');
        if (clearAllBtn) clearAllBtn.onclick = () => this.clearAll();

        const downloadAllBtn = $('convDownloadAllBtn');
        if (downloadAllBtn) downloadAllBtn.onclick = () => this.downloadAll();
    },

    isMediaFile(file) {
        const extension = file.name.split('.').pop().toLowerCase();
        return file.type.startsWith('audio/') || file.type.startsWith('video/') || this.mediaExtensions.has(extension);
    },

    handleFiles(files) {
        const convertibleFiles = files.filter(f => f.type.startsWith('image/') || this.isMediaFile(f));
        if (convertibleFiles.length === 0) return;

        const emptyState = $('convEmptyState');
        if (emptyState) emptyState.style.display = 'none';
        this.updateResultsCount();

        convertibleFiles.forEach(file => {
            const id = 'conv_' + Date.now() + '_' + Math.random().toString(36).substr(2, 9);
            this.createResultCard(id, file);
            this.processingQueue.push({ id, file });
        });

        this.updateResultsCount();
        this.processQueue();
    },

    createResultCard(id, file) {
        const card = document.createElement('div');
        card.className = 'result-card';
        card.id = id;

        this.originalNames.set(id, file.name);

        const origUrl = urlManager.create(file, id + '_src');

        const preview = document.createElement('div');
        preview.className = 'result-preview';

        const previewElement = this.isMediaFile(file)
            ? document.createElement(file.type.startsWith('video/') ? 'video' : 'audio')
            : document.createElement('img');
        previewElement.src = origUrl;
        if (previewElement.tagName === 'IMG') {
            previewElement.alt = '';
        } else {
            previewElement.controls = true;
            previewElement.preload = 'metadata';
        }

        const overlay = document.createElement('div');
        overlay.className = 'loading-overlay';

        const spinner = document.createElement('div');
        spinner.className = 'spinner';

        const label = document.createElement('div');
        label.style.cssText = 'font-size: 0.7rem; color: #9ca3af;';
        label.textContent = 'Конвертация…';

        overlay.appendChild(spinner);
        overlay.appendChild(label);
        preview.appendChild(previewElement);
        preview.appendChild(overlay);

        const info = document.createElement('div');
        info.className = 'result-info';

        const nameSpan = document.createElement('span');
        nameSpan.className = 'result-name';
        nameSpan.dataset.tooltip = file.name;
        nameSpan.textContent = file.name;

        const dlBtn = document.createElement('button');
        dlBtn.className = 'download-btn';
        dlBtn.disabled = true;
        dlBtn.innerHTML = `<svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/></svg>`;

        info.appendChild(nameSpan);
        info.appendChild(dlBtn);
        card.appendChild(preview);
        card.appendChild(info);

        $('convResultsGrid').insertBefore(card, $('convResultsGrid').firstChild);
    },

    async processQueue() {
        if (this.isProcessing || this.processingQueue.length === 0) return;

        this.isProcessing = true;
        const total = this.processingQueue.length + this.results.size;

        $('convBatchProgress').classList.add('show');
        if (this._progressTimer) clearTimeout(this._progressTimer);

        while (this.processingQueue.length > 0) {
            const { id, file } = this.processingQueue.shift();
            const done = this.results.size;

            $('convProgressFill').style.width = `${(done / total) * 100}%`;
            $('convProgressText').textContent = `${done} / ${total}`;

            await this.processFile(id, file);
        }

        $('convProgressFill').style.width = '100%';
        $('convProgressText').textContent = `${this.results.size} / ${this.results.size}`;

        this._progressTimer = setTimeout(() => {
            this._progressTimer = null;
            $('convBatchProgress').classList.remove('show');
        }, 1000);

        this.isProcessing = false;
    },

    async processFile(id, file) {
        const gen = this._gen || 0;
        const fd = new FormData();
        fd.append('image', file);
        fd.append('format', state.converterFormat || 'webp');
        fd.append('quality', state.converterQuality || 90);

        try {
            const res = await fetch('/convert', { method: 'POST', body: fd });
            if (!res.ok) {
                const errData = await res.json().catch(() => ({ error: 'Ошибка сервера' }));
                throw new Error(errData.error || 'Ошибка');
            }

            const blob = await res.blob();

            // clearAll() мог пройти во время запроса (или после проверки, до записи в Map)
            const card = $(id);
            if (!card || gen !== (this._gen || 0)) {
                urlManager.revoke(id);
                urlManager.revoke(id + '_src');
                this.originalNames.delete(id);
                return;
            }

            const ext = state.converterFormat === 'jpg' ? 'jpg' : state.converterFormat;
            const baseName = this.originalNames.get(id) || file.name;
            const newName = baseName.replace(/\.[^.]+$/, '') + '.' + ext;

            this.results.set(id, { blob, name: newName, format: state.converterFormat });

            const preview = card.querySelector('.result-preview');
            const info = card.querySelector('.result-info');

            urlManager.revoke(id);
            const imgUrl = urlManager.create(blob, id);

            const resultElement = state.converterFormat === 'ogg'
                ? document.createElement('audio')
                : document.createElement('img');
            resultElement.src = imgUrl;
            if (resultElement.tagName === 'IMG') {
                resultElement.alt = '';
            } else {
                resultElement.controls = true;
                resultElement.preload = 'metadata';
            }
            preview.innerHTML = '';
            preview.appendChild(resultElement);

            const safeNewName = newName.replace(/</g, '&lt;').replace(/>/g, '&gt;');
            const safeBlobSize = formatSize(blob.size);

            const infoText = document.createElement('div');
            infoText.className = 'result-info-text';
            infoText.innerHTML = `
                <span class="result-name" data-tooltip="${safeNewName}">${safeNewName}</span>
                <div class="result-stats">
                    <span>${safeBlobSize}</span>
                </div>
            `;

            const btnWrap = document.createElement('div');
            btnWrap.style.cssText = 'display:flex;gap:4px;flex-shrink:0;';

            const dlBtn = document.createElement('button');
            dlBtn.className = 'download-btn';
            dlBtn.dataset.tooltip = 'Скачать';
            dlBtn.innerHTML = `<svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/></svg>`;
            dlBtn.onclick = () => this.downloadOne(id);

            btnWrap.appendChild(dlBtn);

            info.innerHTML = '';
            info.appendChild(infoText);
            info.appendChild(btnWrap);

            this.updateDownloadAllBtn();
            this.updateResultsCount();

        } catch (err) {
            const card = $(id);
            if (card) {
                const overlay = card.querySelector('.loading-overlay');
                if (overlay) {
                    const errDiv = document.createElement('div');
                    errDiv.style.color = '#f87171';
                    errDiv.textContent = 'Ошибка';
                    const msgDiv = document.createElement('div');
                    msgDiv.style.cssText = 'font-size: 0.65rem; color: #6b7280;';
                    msgDiv.textContent = err.message;
                    overlay.innerHTML = '';
                    overlay.appendChild(errDiv);
                    overlay.appendChild(msgDiv);
                }
            }
        }
    },

    downloadOne(id) {
        const data = this.results.get(id);
        if (!data) return;
        saveFileWithPicker(data.blob, data.name);
    },

    async downloadAll() {
        if (this.results.size === 0) return;
        const entries = Array.from(this.results.entries());

        const folderPath = await pickFolder();
        if (!folderPath) return;

        if (AppConfig.setLastFolder) AppConfig.setLastFolder('converter', folderPath);

        let saved = 0;
        for (const [id, data] of entries) {
            const ok = await saveToFolder(data.blob, data.name, folderPath);
            if (ok) saved++;
        }
        toast('Сохранено файлов: ' + saved);
    },

    clearAll() {
        this._gen = (this._gen || 0) + 1;
        urlManager.revokeByPrefix('conv_');
        this.results.clear();
        this.originalNames.clear();
        this.processingQueue = [];

        const grid = $('convResultsGrid');
        grid.innerHTML = '';

        const emptyState = document.createElement('div');
        emptyState.className = 'empty-state';
        emptyState.id = 'convEmptyState';
        emptyState.innerHTML = `
            <svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"/></svg>
            <p>Результаты появятся здесь</p>
            <span>Добавьте изображение, аудио или видео</span>
        `;
        grid.appendChild(emptyState);

        this.updateDownloadAllBtn();
        toast('Очищено');
    },

    updateDownloadAllBtn() {
        const dlBtn = $('convDownloadAllBtn');
        const clearBtn = $('convClearAllBtn');
        if (this.results.size > 0) {
            dlBtn.style.display = 'inline-flex';
            dlBtn.innerHTML = `
                <svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/></svg>
                Сохранить все (${this.results.size})
            `;
            clearBtn.style.display = 'inline-flex';
        } else {
            dlBtn.style.display = 'none';
            clearBtn.style.display = 'none';
        }
    },

    updateResultsCount() {
        const cards = document.querySelectorAll('#convResultsGrid .result-card').length;
        const el = $('convResultsCount');
        if (el) el.textContent = cards > 0 ? `Файлов: ${cards}` : '';
    }
};
