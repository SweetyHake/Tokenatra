const Remover = {
    init() {
        this.setupDeviceInfo();
        this.setupFormatControls();
        this.setupDropzone();
        this.setupResultsControls();
        this.setupCompareModal();
    },
    
    setupDeviceInfo() {
        fetch('/device').then(r => r.json()).then(d => {
            const device = d.device || 'CPU';
            if (typeof updateDeviceIndicator === 'function') {
                updateDeviceIndicator(device, d.model_exists !== false);
            }
        }).catch(() => {
            if (typeof updateDeviceIndicator === 'function') updateDeviceIndicator('CPU', false);
        });
    },
    
    setupFormatControls() {
        const formatSelect = $('formatSelect');
        if (formatSelect) {
            var savedFormat = AppConfig.remover.format || 'webp';
            formatSelect.value = savedFormat;
            state.selectedFormat = savedFormat;
            formatSelect.onchange = e => {
                state.selectedFormat = e.target.value;
                AppConfig.setRemover('format', e.target.value);
                toast(`Формат: ${state.selectedFormat.toUpperCase()}`);
            };
        }

        const qualitySlider = $('qualitySlider');
        const qualityValue = $('qualityValue');
        if (qualitySlider) {
            var savedQuality = AppConfig.remover.quality || 90;
            qualitySlider.value = savedQuality;
            state.selectedQuality = savedQuality;
            if (qualityValue) qualityValue.textContent = savedQuality + '%';
            qualitySlider.oninput = e => {
                state.selectedQuality = parseInt(e.target.value);
                if (qualityValue) qualityValue.textContent = state.selectedQuality + '%';
            };
            qualitySlider.onchange = e => {
                AppConfig.setRemover('quality', parseInt(e.target.value));
            };
        }

        const edgeBlurSlider = $('edgeBlurSlider');
        const edgeBlurValue = $('edgeBlurValue');
        if (edgeBlurSlider) {
            var savedEdge = AppConfig.edgeBlur !== undefined ? AppConfig.edgeBlur : 1;
            edgeBlurSlider.value = savedEdge;
            if (edgeBlurValue) edgeBlurValue.textContent = savedEdge;
            edgeBlurSlider.oninput = e => {
                if (edgeBlurValue) edgeBlurValue.textContent = e.target.value;
            };
            edgeBlurSlider.onchange = e => {
                AppConfig.setEdgeBlur(parseFloat(e.target.value));
            };
        }

    },
    
    setupDropzone() {
        const dropzone = $('dropzone');
        const fileInput = $('fileInput');
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
        const clearAllBtn = $('clearAllBtn');
        if (clearAllBtn) clearAllBtn.onclick = () => this.clearAll();

        const downloadAllBtn = $('downloadAllBtn');
        if (downloadAllBtn) downloadAllBtn.onclick = () => this.downloadAll();
    },
    
    setupCompareModal() {
        const closeCompare = $('closeCompare');
        if (closeCompare) {
            closeCompare.onclick = () => this._closeCompareModal();
        }

        const compareModal = $('compareModal');
        if (compareModal) {
            compareModal.onclick = e => {
                if (e.target === compareModal) this._closeCompareModal();
            };
        }
    },

    _closeCompareModal() {
        const modal = $('compareModal');
        if (modal) modal.classList.remove('show');
        if (this._compareId) {
            urlManager.revoke(this._compareId + '_compare');
            this._compareId = null;
        }
        if (this._compareMouseMove) {
            document.removeEventListener('mousemove', this._compareMouseMove);
            document.removeEventListener('mouseup', this._compareMouseUp);
            this._compareMouseMove = null;
            this._compareMouseUp = null;
        }
    },
    
    handleFiles(files) {
        const imageFiles = files.filter(f => f.type.startsWith('image/'));
        if (imageFiles.length === 0) return;

        const emptyState = $('emptyState');
        if (emptyState) emptyState.style.display = 'none';
        this.updateResultsCount();

        imageFiles.forEach(file => {
            const id = 'img_' + Date.now() + '_' + Math.random().toString(36).substr(2, 9);
            this.createResultCard(id, file);
            state.processingQueue.push({ id, file });
        });

        this.updateResultsCount();
        this.processQueue();
    },
    
    createResultCard(id, file) {
        const card = document.createElement('div');
        card.className = 'result-card';
        card.id = id;

        const origUrl = urlManager.create(file, id + '_src');
        state.originalImages.set(id, origUrl);

        const preview = document.createElement('div');
        preview.className = 'result-preview';

        const img = document.createElement('img');
        img.src = origUrl;
        img.alt = '';

        const overlay = document.createElement('div');
        overlay.className = 'loading-overlay';

        const spinner = document.createElement('div');
        spinner.className = 'spinner';

        const label = document.createElement('div');
        label.style.cssText = 'font-size: 0.7rem; color: #9ca3af;';
        label.textContent = 'Обработка…';

        overlay.appendChild(spinner);
        overlay.appendChild(label);
        preview.appendChild(img);
        preview.appendChild(overlay);

        const safeName = file.name.replace(/</g, '&lt;').replace(/>/g, '&gt;');
        const info = document.createElement('div');
        info.className = 'result-info';

        const nameSpan = document.createElement('span');
        nameSpan.className = 'result-name';
        nameSpan.dataset.tooltip = safeName;
        nameSpan.textContent = file.name;

        const dlBtn = document.createElement('button');
        dlBtn.className = 'download-btn';
        dlBtn.disabled = true;
        dlBtn.innerHTML = `<svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/></svg>`;

        info.appendChild(nameSpan);
        info.appendChild(dlBtn);
        card.appendChild(preview);
        card.appendChild(info);

        $('resultsGrid').insertBefore(card, $('resultsGrid').firstChild);
    },
    
    async processQueue() {
        if (state.isProcessing || state.processingQueue.length === 0) return;
        
        state.isProcessing = true;
        if (this._progressTimer) clearTimeout(this._progressTimer);

        $('batchProgress').classList.add('show');

        while (state.processingQueue.length > 0) {
            const { id, file } = state.processingQueue.shift();
            const done = state.results.size;
            // total пересчитывается на каждом шаге: файлы могут быть
            // добавлены в очередь прямо во время обработки пачки
            const total = done + state.processingQueue.length + 1;

            $('progressFill').style.width = `${(done / total) * 100}%`;
            $('progressText').textContent = `${done} / ${total}`;

            await this.processFile(id, file);
        }
        
        $('progressFill').style.width = '100%';
        $('progressText').textContent = `${state.results.size} / ${state.results.size}`;
        
        this._progressTimer = setTimeout(() => {
            this._progressTimer = null;
            $('batchProgress').classList.remove('show');
        }, 1000);
        
        state.isProcessing = false;
    },
    
    async processFile(id, file) {
        const fd = new FormData();
        fd.append('image', file);
        fd.append('format', state.selectedFormat);
        fd.append('quality', state.selectedQuality);
        fd.append('edge_blur', AppConfig.edgeBlur !== undefined ? AppConfig.edgeBlur : 1);

        const gen = this._gen || 0;

        const start = Date.now();

        try {
            const res = await fetch('/process', { method: 'POST', body: fd });
            if (!res.ok) {
                const errData = await res.json().catch(() => ({ error: 'Ошибка сервера' }));
                throw new Error(errData.error || 'Ошибка');
            }

            const blob = await res.blob();
            const time = ((Date.now() - start) / 1000).toFixed(1);

            // clearAll() во время запроса удалил карточку — результат не сохраняем.
            // Генерация: clearAll мог пройти и ПОСЛЕ этой проверки, но до записи в Map.
            const card = $(id);
            if (!card || gen !== (this._gen || 0)) {
                urlManager.revoke(id);
                urlManager.revoke(id + '_src');
                state.originalImages.delete(id);
                return;
            }

            const ext = state.selectedFormat === 'jpg' ? 'jpg' : state.selectedFormat;
            const baseName = file.name.replace(/\.[^.]+$/, '');
            const newName = baseName + '.' + ext;
            state.results.set(id, { blob, name: newName, format: state.selectedFormat });
            if (state.results.size > 100) {
                var oldestKey = state.results.keys().next().value;
                state.results.delete(oldestKey);
                state.originalImages.delete(oldestKey);
                urlManager.revoke(oldestKey);
                urlManager.revoke(oldestKey + '_src');
                urlManager.revoke(oldestKey + '_compare');
                var oldestCard = $(oldestKey);
                if (oldestCard) oldestCard.remove();
                this.updateResultsCount();
            }

            const preview = card.querySelector('.result-preview');
            const info = card.querySelector('.result-info');

            urlManager.revoke(id);
            const imgUrl = urlManager.create(blob, id);

            const img = document.createElement('img');
            img.src = imgUrl;
            img.alt = '';
            preview.innerHTML = '';
            preview.appendChild(img);
            preview.onclick = () => this.showCompare(id);
            preview.style.cursor = 'pointer';
            preview.dataset.tooltip = 'Нажмите для сравнения';

            // Имя файла идёт в атрибут data-tooltip в двойных кавычках —
            // экранируем и кавычки, иначе имя вида x" onmouseover="… внедрит обработчик
            const safeNewName = escapeHtml(newName);
            const safeBlobSize = formatSize(blob.size);

            const infoText = document.createElement('div');
            infoText.className = 'result-info-text';
            infoText.innerHTML = `
                <span class="result-name" data-tooltip="${safeNewName}">${safeNewName}</span>
                <div class="result-stats">
                    <span>${safeBlobSize}</span> • ${time}s
                </div>
            `;

            const btnWrap = document.createElement('div');
            btnWrap.style.cssText = 'display:flex;gap:4px;flex-shrink:0;';

            const copyBtn = document.createElement('button');
            copyBtn.className = 'download-btn';
            copyBtn.dataset.tooltip = 'Копировать в буфер';
            copyBtn.innerHTML = `<svg viewBox="0 0 24 24"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
            copyBtn.onclick = () => this.copyToClipboard(id);

            const editBtn = document.createElement('button');
            editBtn.className = 'download-btn';
            editBtn.dataset.tooltip = 'Открыть в редакторе';
            editBtn.innerHTML = `<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><path d="M12 6v12M6 12h12"/></svg>`;
            editBtn.onclick = () => this.openInEditor(id);

            const dlBtn = document.createElement('button');
            dlBtn.className = 'download-btn';
            dlBtn.dataset.tooltip = 'Скачать';
            dlBtn.innerHTML = `<svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/></svg>`;
            dlBtn.onclick = () => this.downloadOne(id);

            btnWrap.appendChild(copyBtn);
            btnWrap.appendChild(editBtn);
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
    
    showCompare(id) {
        const original = state.originalImages.get(id);
        const result = state.results.get(id);
        if (!original || !result) return;

        if (this._compareId) urlManager.revoke(this._compareId + '_compare');
        const resultUrl = urlManager.create(result.blob, id + '_compare');
        this._compareId = id;

        $('compareBefore').style.backgroundImage = `url(${original})`;
        $('compareAfter').style.backgroundImage = `url(${resultUrl})`;
        $('compareModal').classList.add('show');

        this.initCompareSlider();
    },
    
    initCompareSlider() {
        const slider = $('compareSlider');
        const handle = $('compareHandle');
        const before = $('compareBefore');
        let isDragging = false;

        function updatePosition(x) {
            const rect = slider.getBoundingClientRect();
            let pos = (x - rect.left) / rect.width;
            pos = Math.max(0, Math.min(1, pos));
            handle.style.left = `${pos * 100}%`;
            before.style.clipPath = `inset(0 ${(1 - pos) * 100}% 0 0)`;
        }

        const onMouseMove = e => { if (isDragging) updatePosition(e.clientX); };
        const onMouseUp = () => { isDragging = false; };

        handle.onmousedown = () => { isDragging = true; };

        if (this._compareMouseMove) {
            document.removeEventListener('mousemove', this._compareMouseMove);
            document.removeEventListener('mouseup', this._compareMouseUp);
        }

        this._compareMouseMove = onMouseMove;
        this._compareMouseUp = onMouseUp;

        document.addEventListener('mousemove', onMouseMove);
        document.addEventListener('mouseup', onMouseUp);

        slider.onclick = e => updatePosition(e.clientX);

        updatePosition(slider.getBoundingClientRect().left + slider.offsetWidth / 2);
    },
    
    downloadOne(id) {
        const data = state.results.get(id);
        if (!data) return;
        saveFileWithPicker(data.blob, data.name);
    },
    
    async downloadAll() {
        if (state.results.size === 0) return;
        const entries = Array.from(state.results.entries());

        const folderPath = await pickFolder();
        if (!folderPath) return;

        AppConfig.setLastFolder('remover', folderPath);

        let saved = 0;
        for (const [id, data] of entries) {
            const ok = await saveToFolder(data.blob, data.name, folderPath);
            if (ok) saved++;
        }
        toast('Сохранено файлов: ' + saved);
    },
    
    clearAll() {
        this._gen = (this._gen || 0) + 1;
        urlManager.revokeByPrefix('img_');
        state.results.clear();
        state.originalImages.clear();
        state.processingQueue = [];

        const grid = $('resultsGrid');
        grid.innerHTML = '';

        const emptyState = document.createElement('div');
        emptyState.className = 'empty-state';
        emptyState.id = 'emptyState';
        emptyState.innerHTML = `
            <svg viewBox="0 0 24 24"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>
            <p>Результаты появятся здесь</p>
        `;
        grid.appendChild(emptyState);

        this.updateDownloadAllBtn();
        toast('Очищено');
    },
    
    updateDownloadAllBtn() {
        const dlBtn = $('downloadAllBtn');
        const clearBtn = $('clearAllBtn');
        if (state.results.size > 0) {
            dlBtn.style.display = 'inline-flex';
            dlBtn.innerHTML = `
                <svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/></svg>
                Сохранить все (${state.results.size})
            `;
            clearBtn.style.display = 'inline-flex';
        } else {
            dlBtn.style.display = 'none';
            clearBtn.style.display = 'none';
        }
    },
    
    async copyToClipboard(id) {
        const data = state.results.get(id);
        if (!data) return;

        try {
            let pngBlob = data.blob;
            if (data.format !== 'png') {
                const bitmap = await createImageBitmap(data.blob);
                const canvas = document.createElement('canvas');
                canvas.width = bitmap.width;
                canvas.height = bitmap.height;
                const ctx = canvas.getContext('2d');
                ctx.drawImage(bitmap, 0, 0);
                bitmap.close();
                pngBlob = await new Promise(resolve => canvas.toBlob(resolve, 'image/png'));
            }
            await navigator.clipboard.write([
                new ClipboardItem({ 'image/png': pngBlob })
            ]);
            toast('Скопировано в буфер');
        } catch {
            toast('Не удалось скопировать', true);
        }
    },

    openInEditor(id) {
        const data = state.results.get(id);
        if (!data) { toast('Результат не найден', true); return; }
        const file = new File([data.blob], data.name, { type: data.blob.type });
        // Как в initTabs(): классы + aria-hidden + inert — иначе tokenPanel
        // с классом .active остаётся visibility:hidden (CSS .panel[aria-hidden=true])
        document.querySelectorAll('.nav-btn[data-mode]').forEach(function(t) {
            t.classList.toggle('active', t.dataset.mode === 'token');
            t.setAttribute('aria-selected', String(t.dataset.mode === 'token'));
        });
        document.querySelectorAll('.panel').forEach(function(p) {
            const active = p.id === 'tokenPanel';
            p.classList.toggle('active', active);
            p.setAttribute('aria-hidden', String(!active));
            p.inert = !active;
        });
        var panel = $('tokenPanel');
        if (panel) panel.focus?.({ preventScroll: true });
        TokenCanvas.loadImage(file);
        toast('Открыто в редакторе');
    },
    
    updateResultsCount() {
        const cards = document.querySelectorAll('#resultsGrid .result-card').length;
        const el = $('resultsCount');
        if (el) el.textContent = cards > 0 ? `Файлов: ${cards}` : '';
    }
};
