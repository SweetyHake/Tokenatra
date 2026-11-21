/**
 * tokenMedia.js — поддержка анимированных изображений (GIF, animated WebP, APNG)
 * и видео (WebM, MP4, MOV, MKV, AVI, OGV, M4V) с живым предпросмотром на канвасе,
 * покадровым удалением фона и оффлайн-экспортом в WebM с альфа-каналом (VP9/VP8 Alpha).
 */
(function() {
    'use strict';

    var VIDEO_EXTS = new Set(['webm', 'mp4', 'mov', 'm4v', 'ogv', 'mkv', 'avi', 'mpeg', 'mpg']);
    var ANIM_CANDIDATE_EXTS = new Set(['gif', 'apng', 'webp', 'png']);
    var IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'avif', 'apng']);
    var MAX_MEDIA_DIM = 2048;
    var MAX_DECODE_FRAMES = 600;
    var DEFAULT_VIDEO_FPS = 30;
    var MAX_EXPORT_FPS = 30;

    function getExt(name) {
        if (!name) return '';
        var idx = name.lastIndexOf('.');
        return idx >= 0 ? name.slice(idx + 1).toLowerCase() : '';
    }

    function isVideoFile(file) {
        if (!file) return false;
        if (file.type && file.type.startsWith('video/')) return true;
        return VIDEO_EXTS.has(getExt(file.name));
    }

    function isGifFile(file) {
        if (!file) return false;
        if (file.type === 'image/gif') return true;
        return getExt(file.name) === 'gif';
    }

    function isAnimatedCandidate(file) {
        if (!file) return false;
        if (isVideoFile(file)) return true;
        if (file.type === 'image/gif' || file.type === 'image/apng' || file.type === 'image/webp' || file.type === 'image/png') {
            return true;
        }
        return ANIM_CANDIDATE_EXTS.has(getExt(file.name));
    }

    function isSupportedFile(file) {
        if (!file) return false;
        if (file.type && (file.type.startsWith('image/') || file.type.startsWith('video/'))) return true;
        var ext = getExt(file.name);
        return IMAGE_EXTS.has(ext) || VIDEO_EXTS.has(ext);
    }

    function guessMime(file) {
        if (file && file.type && file.type !== 'application/octet-stream') return file.type;
        var ext = getExt(file && file.name);
        switch (ext) {
            case 'gif': return 'image/gif';
            case 'webp': return 'image/webp';
            case 'png': return 'image/png';
            case 'apng': return 'image/apng';
            case 'webm': return 'video/webm';
            case 'mp4': case 'm4v': return 'video/mp4';
            case 'mov': return 'video/quicktime';
            case 'ogv': return 'video/ogg';
            case 'mkv': return 'video/x-matroska';
            case 'avi': return 'video/x-msvideo';
            default: return '';
        }
    }

    // =========================================================================
    // EBML / WebM Alpha Muxer (VP9 / VP8 + AlphaMode=1 + BlockAdditions + Cues)
    // =========================================================================

    function ebmlVintSize(value) {
        // Unsigned VINT for EBML element payload size (1..8 bytes)
        if (value < 0x7F) {
            return new Uint8Array([0x80 | value]);
        }
        if (value < 0x3FFF) {
            return new Uint8Array([0x40 | (value >> 8), value & 0xFF]);
        }
        if (value < 0x1FFFFF) {
            return new Uint8Array([0x20 | (value >> 16), (value >> 8) & 0xFF, value & 0xFF]);
        }
        if (value < 0x0FFFFFFF) {
            return new Uint8Array([
                0x10 | (value >>> 24),
                (value >>> 16) & 0xFF,
                (value >>> 8) & 0xFF,
                value & 0xFF
            ]);
        }
        // Up to 56-bit via BigInt
        var big = BigInt(value);
        for (var len = 5; len <= 8; len++) {
            var max = (1n << BigInt(len * 7)) - 2n;
            if (big <= max) {
                var out = new Uint8Array(len);
                for (var i = len - 1; i >= 0; i--) {
                    out[i] = Number(big & 0xFFn);
                    big >>= 8n;
                }
                out[0] |= (1 << (8 - len));
                return out;
            }
        }
        throw new Error('EBML size overflow');
    }

    function ebmlIdBytes(id) {
        if (id <= 0xFF) return new Uint8Array([id]);
        if (id <= 0xFFFF) return new Uint8Array([(id >>> 8) & 0xFF, id & 0xFF]);
        if (id <= 0xFFFFFF) return new Uint8Array([(id >>> 16) & 0xFF, (id >>> 8) & 0xFF, id & 0xFF]);
        return new Uint8Array([(id >>> 24) & 0xFF, (id >>> 16) & 0xFF, (id >>> 8) & 0xFF, id & 0xFF]);
    }

    function concatBytes(parts) {
        var total = 0;
        for (var i = 0; i < parts.length; i++) total += parts[i].byteLength;
        var out = new Uint8Array(total);
        var offset = 0;
        for (var j = 0; j < parts.length; j++) {
            out.set(parts[j], offset);
            offset += parts[j].byteLength;
        }
        return out;
    }

    function ebmlElem(id, data) {
        var payload = data instanceof Uint8Array ? data : concatBytes(data);
        return concatBytes([ebmlIdBytes(id), ebmlVintSize(payload.byteLength), payload]);
    }

    function ebmlUint(id, val) {
        var big = BigInt(Math.max(0, Math.round(val)));
        var bytes = [];
        do {
            bytes.unshift(Number(big & 0xFFn));
            big >>= 8n;
        } while (big > 0n);
        return ebmlElem(id, new Uint8Array(bytes));
    }

    function ebmlSint(id, val) {
        var v = Math.round(val);
        var bytes;
        if (v >= -128 && v <= 127) {
            bytes = new Uint8Array([v & 0xFF]);
        } else if (v >= -32768 && v <= 32767) {
            bytes = new Uint8Array([(v >> 8) & 0xFF, v & 0xFF]);
        } else if (v >= -8388608 && v <= 8388607) {
            bytes = new Uint8Array([(v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF]);
        } else {
            bytes = new Uint8Array([(v >> 24) & 0xFF, (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF]);
        }
        return ebmlElem(id, bytes);
    }

    function ebmlFloat64(id, val) {
        var buf = new ArrayBuffer(8);
        new DataView(buf).setFloat64(0, val, false);
        return ebmlElem(id, new Uint8Array(buf));
    }

    function ebmlStr(id, str) {
        var out = new Uint8Array(str.length);
        for (var i = 0; i < str.length; i++) out[i] = str.charCodeAt(i) & 0x7F;
        return ebmlElem(id, out);
    }

    function buildBlockPayload(relTimeMs, flags, frameBytes) {
        var out = new Uint8Array(4 + frameBytes.byteLength);
        out[0] = 0x81; // TrackNumber = 1 (VINT)
        var t = Math.max(-32768, Math.min(32767, Math.round(relTimeMs)));
        out[1] = (t >> 8) & 0xFF;
        out[2] = t & 0xFF;
        out[3] = flags & 0xFF;
        out.set(frameBytes, 4);
        return out;
    }

    /**
     * Muxes encoded VP9/VP8 chunks (with optional VP9/VP8 alpha chunks) into a WebM Blob.
     * packets: Array<{ tsMs: number, durMs: number, key: boolean, data: Uint8Array, alpha?: Uint8Array }>
     */
    function muxWebm(opts) {
        var width = opts.width;
        var height = opts.height;
        var codecId = opts.codecId || 'V_VP9';
        var hasAlpha = !!opts.hasAlpha;
        var packets = opts.packets || [];
        var totalDurationMs = Math.max(1, Math.round(opts.durationMs || 1));

        // 1. EBML Header
        var ebmlHeader = ebmlElem(0x1A45DFA3, [
            ebmlUint(0x4286, 1),          // EBMLVersion
            ebmlUint(0x42F7, 1),          // EBMLReadVersion
            ebmlUint(0x42F2, 4),          // EBMLMaxIDLength
            ebmlUint(0x42F3, 8),          // EBMLMaxSizeLength
            ebmlStr(0x4282, 'webm'),      // DocType
            ebmlUint(0x4287, 4),          // DocTypeVersion
            ebmlUint(0x4285, 2)           // DocTypeReadVersion
        ]);

        // 2. Info
        var info = ebmlElem(0x1549A966, [
            ebmlUint(0x2AD7B1, 1000000),      // TimecodeScale = 1ms
            ebmlStr(0x4D80, 'Tokenatra'),     // MuxingApp
            ebmlStr(0x5741, 'Tokenatra'),     // WritingApp
            ebmlFloat64(0x4489, totalDurationMs) // Duration in TimecodeScale ticks (ms)
        ]);

        // 3. Tracks
        var videoFields = [
            ebmlUint(0xB0, width),
            ebmlUint(0xBA, height)
        ];
        if (hasAlpha) {
            videoFields.push(ebmlUint(0x53C0, 1)); // AlphaMode = 1
        }
        var trackEntry = ebmlElem(0xAE, [
            ebmlUint(0xD7, 1),            // TrackNumber
            ebmlUint(0x73C5, 1),          // TrackUID
            ebmlUint(0x83, 1),            // TrackType = video
            ebmlStr(0x86, codecId),       // CodecID
            ebmlElem(0xE0, videoFields)   // Video
        ]);
        var tracks = ebmlElem(0x1654AE6B, [trackEntry]);

        // 4. Clusters (start a new cluster on every keyframe or if relTime > 30000ms)
        var clusters = [];
        var cueEntries = [];
        var curClusterTime = 0;
        var curClusterBlocks = [];
        var prevTsMs = 0;

        function flushCluster() {
            if (curClusterBlocks.length === 0) return;
            var clusterParts = [ebmlUint(0xE7, curClusterTime)];
            for (var i = 0; i < curClusterBlocks.length; i++) {
                clusterParts.push(curClusterBlocks[i]);
            }
            clusters.push({
                timeMs: curClusterTime,
                bytes: ebmlElem(0x1F43B675, clusterParts)
            });
            curClusterBlocks = [];
        }

        for (var i = 0; i < packets.length; i++) {
            var pkt = packets[i];
            var tsMs = Math.max(0, Math.round(pkt.tsMs));
            var durMs = Math.max(1, Math.round(pkt.durMs || 33));
            var startNew = curClusterBlocks.length === 0 || pkt.key || (tsMs - curClusterTime) > 30000;
            if (startNew) {
                flushCluster();
                curClusterTime = tsMs;
            }
            var relMs = tsMs - curClusterTime;
            if (hasAlpha && pkt.alpha && pkt.alpha.byteLength > 0) {
                var groupParts = [
                    ebmlElem(0xA1, buildBlockPayload(relMs, 0x00, pkt.data)),
                    ebmlUint(0x9B, durMs),
                    ebmlElem(0x75A1, [
                        ebmlElem(0xA6, [
                            ebmlUint(0xEE, 1),
                            ebmlElem(0xA5, pkt.alpha)
                        ])
                    ])
                ];
                if (!pkt.key && curClusterBlocks.length > 0) {
                    var refDelta = prevTsMs - tsMs;
                    if (refDelta >= 0) refDelta = -1;
                    groupParts.push(ebmlSint(0xFB, refDelta));
                }
                curClusterBlocks.push(ebmlElem(0xA0, groupParts));
            } else {
                var flags = pkt.key ? 0x80 : 0x00;
                curClusterBlocks.push(ebmlElem(0xA3, buildBlockPayload(relMs, flags, pkt.data)));
            }
            prevTsMs = tsMs;
        }
        flushCluster();

        // 5. Cues (relative to start of Segment payload)
        var offsetInSegment = info.byteLength + tracks.byteLength;
        var cuePoints = [];
        for (var c = 0; c < clusters.length; c++) {
            cuePoints.push(ebmlElem(0xBB, [
                ebmlUint(0xB3, clusters[c].timeMs),
                ebmlElem(0xB7, [
                    ebmlUint(0xF7, 1),
                    ebmlUint(0xF1, offsetInSegment)
                ])
            ]));
            offsetInSegment += clusters[c].bytes.byteLength;
        }
        var cues = cuePoints.length ? ebmlElem(0x1C53BB6B, cuePoints) : new Uint8Array(0);

        var segParts = [info, tracks];
        for (var k = 0; k < clusters.length; k++) segParts.push(clusters[k].bytes);
        if (cues.byteLength) segParts.push(cues);

        var segment = ebmlElem(0x18538067, segParts);
        return new Blob([ebmlHeader, segment], { type: 'video/webm' });
    }

    // =========================================================================
    // WebCodecs VideoEncoder (VP9 / VP8 + Alpha) with MediaRecorder fallback
    // =========================================================================

    function pickBitrate(width, height, fps, compression) {
        var q = Math.max(10, Math.min(100, parseInt(compression) || 95));
        var qRatio = (q - 10) / 90;
        var pixelsPerSec = width * height * Math.max(10, Math.min(60, fps || 30));
        // Scales smoothly: 512x512@30 @95% -> ~2.5 Mbps; 1024x1024@30 @95% -> ~10 Mbps
        var bpp = 0.08 + 0.24 * qRatio;
        return Math.round(Math.max(1000000, Math.min(24000000, pixelsPerSec * bpp)));
    }

    async function pickWebCodecsConfig(width, height, fps, bitrate) {
        if (typeof VideoEncoder === 'undefined' || typeof VideoFrame === 'undefined') return null;
        var candidates = [
            { codec: 'vp09.00.41.08', codecId: 'V_VP9' },
            { codec: 'vp09.00.31.08', codecId: 'V_VP9' },
            { codec: 'vp09.00.10.08', codecId: 'V_VP9' },
            { codec: 'vp8', codecId: 'V_VP8' }
        ];
        var hwModes = ['prefer-software', 'no-preference'];
        for (var c = 0; c < candidates.length; c++) {
            for (var h = 0; h < hwModes.length; h++) {
                var cfg = {
                    codec: candidates[c].codec,
                    width: width,
                    height: height,
                    bitrate: bitrate,
                    framerate: Math.max(1, Math.min(60, Math.round(fps || 30))),
                    hardwareAcceleration: hwModes[h],
                    latencyMode: 'quality'
                };
                try {
                    var sup = await VideoEncoder.isConfigSupported(cfg);
                    if (sup && sup.supported) {
                        return { config: sup.config || cfg, codecId: candidates[c].codecId };
                    }
                } catch (err) { /* try next */ }
            }
        }
        return null;
    }

    function waitEncoderQueue(encoder, maxQueue) {
        if (!encoder || encoder.encodeQueueSize <= maxQueue) return Promise.resolve();
        return new Promise(function(resolve) {
            var done = false;
            var finish = function() {
                if (done) return;
                done = true;
                resolve();
            };
            if (typeof encoder.addEventListener === 'function') {
                encoder.addEventListener('dequeue', function onDq() {
                    if (encoder.encodeQueueSize <= maxQueue) {
                        encoder.removeEventListener('dequeue', onDq);
                        finish();
                    }
                });
            }
            setTimeout(finish, 30);
        });
    }

    /**
     * Encodes a sequence of frames produced by getFrame(index) into a WebM Blob.
     * opts: {
     *   width, height, frameCount,
     *   getFrame: async (idx) => { canvas, durationMs },
     *   fps, compression, hasAlpha,
     *   onProgress?: (done, total) => void,
     *   signal?: AbortSignal
     * }
     */
    async function encodeFramesToWebm(opts) {
        var width = opts.width & ~1;
        var height = opts.height & ~1;
        if (width < 2) width = 2;
        if (height < 2) height = 2;
        var frameCount = opts.frameCount;
        var fps = opts.fps || DEFAULT_VIDEO_FPS;
        var hasAlpha = opts.hasAlpha !== false;
        var bitrate = pickBitrate(width, height, fps, opts.compression);
        var signal = opts.signal || null;

        var picked = await pickWebCodecsConfig(width, height, fps, bitrate);
        if (picked) {
            return await encodeWithWebCodecs({
                width: width,
                height: height,
                frameCount: frameCount,
                fps: fps,
                hasAlpha: hasAlpha,
                picked: picked,
                bitrate: bitrate,
                getFrame: opts.getFrame,
                onProgress: opts.onProgress,
                signal: signal
            });
        }
        return await encodeWithMediaRecorder({
            width: width,
            height: height,
            frameCount: frameCount,
            fps: fps,
            bitrate: bitrate,
            getFrame: opts.getFrame,
            onProgress: opts.onProgress,
            signal: signal
        });
    }

    async function encodeWithWebCodecs(opts) {
        var width = opts.width;
        var height = opts.height;
        var frameCount = opts.frameCount;
        var hasAlpha = opts.hasAlpha;
        var signal = opts.signal;
        var keyInterval = Math.max(15, Math.min(60, Math.round((opts.fps || 30) * 2)));

        var colorChunks = [];
        var alphaChunks = [];
        var encErr = null;

        var colorEncoder = new VideoEncoder({
            output: function(chunk) {
                var buf = new Uint8Array(chunk.byteLength);
                chunk.copyTo(buf);
                colorChunks.push({
                    data: buf,
                    key: chunk.type === 'key',
                    tsUs: chunk.timestamp,
                    durUs: chunk.duration || 0
                });
            },
            error: function(err) { encErr = err; }
        });
        colorEncoder.configure(opts.picked.config);

        var alphaEncoder = null;
        if (hasAlpha) {
            var alphaCfg = Object.assign({}, opts.picked.config, {
                bitrate: Math.max(800000, Math.round(opts.bitrate * 0.75))
            });
            alphaEncoder = new VideoEncoder({
                output: function(chunk) {
                    var buf = new Uint8Array(chunk.byteLength);
                    chunk.copyTo(buf);
                    alphaChunks.push({
                        data: buf,
                        key: chunk.type === 'key',
                        tsUs: chunk.timestamp
                    });
                },
                error: function(err) { encErr = err; }
            });
            alphaEncoder.configure(alphaCfg);
        }

        // Reusable scratch buffers
        var workCanvas = document.createElement('canvas');
        workCanvas.width = width;
        workCanvas.height = height;
        var workCtx = workCanvas.getContext('2d', { willReadFrequently: true });

        var pxCount = width * height;
        var rgbBuf = new Uint8Array(pxCount * 4);
        var uvSize = (width >> 1) * (height >> 1);
        var i420AlphaBuf = hasAlpha ? new Uint8Array(pxCount + uvSize * 2) : null;
        if (i420AlphaBuf) i420AlphaBuf.fill(128, pxCount);

        var frameMeta = [];
        var curTsUs = 0;

        try {
            for (var i = 0; i < frameCount; i++) {
                if (signal && signal.aborted) throw new DOMException('Aborted', 'AbortError');
                if (encErr) throw encErr;

                var f = await opts.getFrame(i);
                if (signal && signal.aborted) throw new DOMException('Aborted', 'AbortError');
                if (!f || !f.canvas) continue;

                var durMs = Math.max(16, Math.round(f.durationMs || (1000 / (opts.fps || 30))));
                var durUs = durMs * 1000;
                var isKey = (i === 0) || (i % keyInterval === 0);

                workCtx.clearRect(0, 0, width, height);
                if (f.canvas.width === width && f.canvas.height === height) {
                    workCtx.drawImage(f.canvas, 0, 0);
                } else {
                    workCtx.imageSmoothingEnabled = true;
                    workCtx.imageSmoothingQuality = 'high';
                    workCtx.drawImage(f.canvas, 0, 0, width, height);
                }

                // Canvas2D getImageData returns straight (unpremultiplied) RGBA:
                // zero RGB where alpha==0 to avoid color bleed, keep straight RGB elsewhere,
                // and write raw 0..255 alpha into Y-plane of full-range I420.
                var rgba = workCtx.getImageData(0, 0, width, height).data;
                if (hasAlpha) {
                    for (var p = 0, o = 0; p < pxCount; p++, o += 4) {
                        var a = rgba[o + 3];
                        if (a <= 2) {
                            i420AlphaBuf[p] = 0;
                            rgbBuf[o] = 0;
                            rgbBuf[o + 1] = 0;
                            rgbBuf[o + 2] = 0;
                            rgbBuf[o + 3] = 255;
                        } else {
                            i420AlphaBuf[p] = a;
                            rgbBuf[o] = rgba[o];
                            rgbBuf[o + 1] = rgba[o + 1];
                            rgbBuf[o + 2] = rgba[o + 2];
                            rgbBuf[o + 3] = 255;
                        }
                    }
                } else {
                    for (var p2 = 0, o2 = 0; p2 < pxCount; p2++, o2 += 4) {
                        rgbBuf[o2] = rgba[o2];
                        rgbBuf[o2 + 1] = rgba[o2 + 1];
                        rgbBuf[o2 + 2] = rgba[o2 + 2];
                        rgbBuf[o2 + 3] = 255;
                    }
                }

                var colorFrame = new VideoFrame(rgbBuf, {
                    format: 'RGBA',
                    codedWidth: width,
                    codedHeight: height,
                    timestamp: curTsUs,
                    duration: durUs
                });
                colorEncoder.encode(colorFrame, { keyFrame: isKey });
                colorFrame.close();

                if (alphaEncoder) {
                    var alphaFrame = new VideoFrame(i420AlphaBuf, {
                        format: 'I420',
                        codedWidth: width,
                        codedHeight: height,
                        timestamp: curTsUs,
                        duration: durUs,
                        colorSpace: {
                            primaries: 'bt709',
                            transfer: 'bt709',
                            matrix: 'bt709',
                            fullRange: true
                        }
                    });
                    alphaEncoder.encode(alphaFrame, { keyFrame: isKey });
                    alphaFrame.close();
                }

                frameMeta.push({ tsMs: curTsUs / 1000, durMs: durMs });
                curTsUs += durUs;

                if (opts.onProgress) opts.onProgress(i + 1, frameCount);

                await waitEncoderQueue(colorEncoder, 6);
                if (alphaEncoder) await waitEncoderQueue(alphaEncoder, 6);
                if ((i & 3) === 3) await new Promise(function(r) { setTimeout(r, 0); });
            }

            await colorEncoder.flush();
            if (alphaEncoder) await alphaEncoder.flush();
            if (encErr) throw encErr;
        } finally {
            try { colorEncoder.close(); } catch (e) {}
            if (alphaEncoder) { try { alphaEncoder.close(); } catch (e) {} }
            workCanvas.width = 1;
            workCanvas.height = 1;
        }

        var count = colorChunks.length;
        if (count === 0) throw new Error('Не удалось закодировать кадры');

        var packets = [];
        for (var j = 0; j < count; j++) {
            var meta = frameMeta[j] || {
                tsMs: colorChunks[j].tsUs / 1000,
                durMs: Math.max(16, (colorChunks[j].durUs || 33000) / 1000)
            };
            packets.push({
                tsMs: meta.tsMs,
                durMs: meta.durMs,
                key: colorChunks[j].key,
                data: colorChunks[j].data,
                alpha: (hasAlpha && alphaChunks[j]) ? alphaChunks[j].data : null
            });
        }

        return muxWebm({
            width: width,
            height: height,
            codecId: opts.picked.codecId,
            hasAlpha: hasAlpha && alphaChunks.length === count,
            packets: packets,
            durationMs: curTsUs / 1000
        });
    }

    async function encodeWithMediaRecorder(opts) {
        if (typeof MediaRecorder === 'undefined') {
            throw new Error('Кодирование WebM не поддерживается этим браузером');
        }
        var mimeCandidates = [
            'video/webm;codecs=vp9',
            'video/webm;codecs=vp8',
            'video/webm'
        ];
        var mime = mimeCandidates.find(function(m) { return MediaRecorder.isTypeSupported(m); });
        if (!mime) throw new Error('Кодирование WebM не поддерживается этим браузером');

        var recCanvas = document.createElement('canvas');
        recCanvas.width = opts.width;
        recCanvas.height = opts.height;
        var recCtx = recCanvas.getContext('2d');
        var stream = recCanvas.captureStream(opts.fps || 30);
        var chunks = [];
        var recorder = new MediaRecorder(stream, {
            mimeType: mime,
            videoBitsPerSecond: opts.bitrate
        });
        recorder.ondataavailable = function(e) {
            if (e.data && e.data.size > 0) chunks.push(e.data);
        };
        var stopped = new Promise(function(resolve, reject) {
            recorder.onstop = function() { resolve(new Blob(chunks, { type: 'video/webm' })); };
            recorder.onerror = function(e) { reject(e.error || new Error('MediaRecorder error')); };
        });

        recorder.start(100);
        try {
            for (var i = 0; i < opts.frameCount; i++) {
                if (opts.signal && opts.signal.aborted) {
                    recorder.stop();
                    throw new DOMException('Aborted', 'AbortError');
                }
                var f = await opts.getFrame(i);
                if (f && f.canvas) {
                    recCtx.clearRect(0, 0, opts.width, opts.height);
                    recCtx.drawImage(f.canvas, 0, 0, opts.width, opts.height);
                }
                if (opts.onProgress) opts.onProgress(i + 1, opts.frameCount);
                var delay = Math.max(16, Math.round((f && f.durationMs) || (1000 / (opts.fps || 30))));
                await new Promise(function(r) { setTimeout(r, delay); });
            }
            recorder.stop();
            return await stopped;
        } finally {
            stream.getTracks().forEach(function(t) { t.stop(); });
            recCanvas.width = 1;
            recCanvas.height = 1;
        }
    }

    // =========================================================================
    // Animated Image & Video Decoding
    // =========================================================================

    function computeCappedSize(w, h) {
        if (!w || !h) return { w: 1, h: 1 };
        var maxDim = Math.max(w, h);
        if (maxDim <= MAX_MEDIA_DIM) return { w: w, h: h };
        var ratio = MAX_MEDIA_DIM / maxDim;
        return {
            w: Math.max(2, Math.round(w * ratio) & ~1),
            h: Math.max(2, Math.round(h * ratio) & ~1)
        };
    }

    async function decodeAnimatedImage(file) {
        if (typeof ImageDecoder === 'undefined') return null;
        var mime = guessMime(file);
        if (!mime || !mime.startsWith('image/')) return null;
        try {
            var supported = await ImageDecoder.isTypeSupported(mime);
            if (!supported) return null;
        } catch (e) {
            return null;
        }

        var buffer = await file.arrayBuffer();
        var decoder = new ImageDecoder({ data: buffer, type: mime });
        try {
            await decoder.tracks.ready;
            var track = decoder.tracks.selectedTrack;
            if (!track || track.frameCount <= 1) {
                return null; // Static image — let normal <img> path handle it
            }

            var totalFrames = Math.min(track.frameCount, MAX_DECODE_FRAMES);
            var step = track.frameCount > MAX_DECODE_FRAMES
                ? track.frameCount / MAX_DECODE_FRAMES
                : 1;

            var frames = [];
            var outW = 0, outH = 0;
            var totalDurationMs = 0;

            for (var k = 0; k < totalFrames; k++) {
                var srcIdx = Math.min(track.frameCount - 1, Math.floor(k * step));
                var res = await decoder.decode({ frameIndex: srcIdx });
                var vf = res.image;
                try {
                    if (!outW) {
                        var capped = computeCappedSize(vf.displayWidth || vf.codedWidth, vf.displayHeight || vf.codedHeight);
                        outW = capped.w;
                        outH = capped.h;
                    }
                    var durMs = vf.duration ? Math.round(vf.duration / 1000) : 100;
                    if (!durMs || durMs < 16) durMs = 100;
                    if (step > 1) durMs = Math.round(durMs * step);

                    var bitmap = await createImageBitmap(vf, {
                        resizeWidth: outW,
                        resizeHeight: outH,
                        resizeQuality: 'high'
                    });
                    frames.push({ bitmap: bitmap, durationMs: durMs });
                    totalDurationMs += durMs;
                } finally {
                    vf.close();
                }
            }

            if (frames.length <= 1) {
                frames.forEach(function(fr) { if (fr.bitmap && fr.bitmap.close) fr.bitmap.close(); });
                return null;
            }

            var avgFps = Math.round((frames.length * 1000) / Math.max(1, totalDurationMs));
            return {
                kind: 'frames',
                width: outW,
                height: outH,
                frames: frames,
                durationMs: totalDurationMs,
                fps: Math.max(1, Math.min(60, avgFps || 15))
            };
        } finally {
            try { decoder.close(); } catch (e) {}
        }
    }

    function loadVideoElementFromBlob(blob) {
        return new Promise(function(resolve, reject) {
            var url = URL.createObjectURL(blob);
            var video = document.createElement('video');
            video.muted = true;
            video.loop = true;
            video.playsInline = true;
            video.preload = 'auto';
            video.crossOrigin = 'anonymous';

            var settled = false;
            var timer = setTimeout(function() {
                if (settled) return;
                settled = true;
                URL.revokeObjectURL(url);
                reject(new Error('Таймаут загрузки видео'));
            }, 15000);

            function cleanupListeners() {
                clearTimeout(timer);
                video.onloadeddata = null;
                video.onerror = null;
            }

            video.onloadeddata = function() {
                if (settled) return;
                if (!video.videoWidth || !video.videoHeight) {
                    settled = true;
                    cleanupListeners();
                    URL.revokeObjectURL(url);
                    reject(new Error('Видео не содержит видеопотока'));
                    return;
                }
                settled = true;
                cleanupListeners();
                resolve({ video: video, url: url });
            };

            video.onerror = function() {
                if (settled) return;
                settled = true;
                cleanupListeners();
                URL.revokeObjectURL(url);
                reject(new Error('Браузер не смог декодировать видео'));
            };

            video.src = url;
            video.load();
        });
    }

    async function decodeVideoFile(file) {
        var loaded = null;
        try {
            loaded = await loadVideoElementFromBlob(file);
        } catch (nativeErr) {
            // Fallback: try server-side FFmpeg transcoding for MKV/AVI/exotic codecs
            if (!(window.TokenTransport && TokenTransport.active())) {
                try {
                    var fd = new FormData();
                    fd.append('file', file, file.name || 'video.mp4');
                    var res = await fetch('/transcode_video', { method: 'POST', body: fd });
                    if (res.ok) {
                        var webmBlob = await res.blob();
                        loaded = await loadVideoElementFromBlob(webmBlob);
                    }
                } catch (e) { /* fall through */ }
            }
            if (!loaded) throw nativeErr;
        }

        var video = loaded.video;
        var capped = computeCappedSize(video.videoWidth, video.videoHeight);
        var durationSec = (isFinite(video.duration) && video.duration > 0) ? video.duration : 1;

        return {
            kind: 'video',
            video: video,
            videoUrl: loaded.url,
            width: capped.w,
            height: capped.h,
            durationMs: Math.max(100, Math.round(durationSec * 1000)),
            fps: DEFAULT_VIDEO_FPS,
            measuredFps: null
        };
    }

    function seekVideoTo(video, timeSec) {
        return new Promise(function(resolve) {
            var target = Math.max(0, Math.min(Math.max(0, (video.duration || 0) - 0.002), timeSec));
            if (Math.abs(video.currentTime - target) < 0.0005 && video.readyState >= 2) {
                resolve();
                return;
            }
            var done = false;
            var finish = function() {
                if (done) return;
                done = true;
                video.removeEventListener('seeked', onSeeked);
                resolve();
            };
            var onSeeked = function() {
                // Wait one paint frame so WebView2 flushes the decoded texture
                requestAnimationFrame(finish);
            };
            video.addEventListener('seeked', onSeeked, { once: true });
            setTimeout(finish, 400);
            try {
                video.currentTime = target;
            } catch (e) {
                finish();
            }
        });
    }

    // =========================================================================
    // Active Media Session Controller (playback, scrubbing, frame extraction)
    // =========================================================================

    function createMediaSession(decoded, rawFile) {
        var frameCanvas = document.createElement('canvas');
        frameCanvas.width = decoded.width;
        frameCanvas.height = decoded.height;
        var frameCtx = frameCanvas.getContext('2d');

        var session = {
            kind: decoded.kind,              // 'frames' | 'video'
            rawFile: rawFile,
            width: decoded.width,
            height: decoded.height,
            durationMs: decoded.durationMs,
            fps: decoded.fps,
            frameCanvas: frameCanvas,
            frameCtx: frameCtx,

            // Frame-based original & cutout storage
            originalFrames: decoded.kind === 'frames' ? decoded.frames : null,
            cutoutFrames: null,              // populated after AI removeBg
            showingCutout: false,

            // Video element (when kind === 'video')
            video: decoded.video || null,
            videoUrl: decoded.videoUrl || null,

            // Playback state
            playing: true,
            userPaused: false,
            currentFrameIndex: 0,
            currentTimeMs: 0,
            _rafId: null,
            _vfcId: null,
            _lastTick: 0,
            _accumMs: 0,
            _lastVideoTime: -1,
            _lastVfcMediaTime: null,
            _fpsSamples: [],
            _disposed: false,
            _onFrameCallback: null,

            activeFrames: function() {
                if (this.showingCutout && this.cutoutFrames && this.cutoutFrames.length) {
                    return this.cutoutFrames;
                }
                return this.originalFrames;
            },

            isUsingFrames: function() {
                return !!(this.activeFrames() && this.activeFrames().length);
            },

            drawCurrentToCanvas: function() {
                if (this._disposed) return;
                var ctx = this.frameCtx;
                ctx.clearRect(0, 0, this.width, this.height);
                var frames = this.activeFrames();
                if (frames && frames.length) {
                    var idx = Math.max(0, Math.min(frames.length - 1, this.currentFrameIndex));
                    var fr = frames[idx];
                    if (fr && fr.bitmap) {
                        ctx.drawImage(fr.bitmap, 0, 0, this.width, this.height);
                    }
                } else if (this.video && this.video.readyState >= 2) {
                    ctx.drawImage(this.video, 0, 0, this.width, this.height);
                }
            },

            drawFrameIndexToCanvas: function(idx, useCutoutOverride) {
                if (this._disposed) return;
                var frames = (useCutoutOverride === true)
                    ? this.cutoutFrames
                    : (useCutoutOverride === false ? this.originalFrames : this.activeFrames());
                if (!frames || !frames.length) return;
                var clamped = Math.max(0, Math.min(frames.length - 1, idx));
                var ctx = this.frameCtx;
                ctx.clearRect(0, 0, this.width, this.height);
                if (frames[clamped] && frames[clamped].bitmap) {
                    ctx.drawImage(frames[clamped].bitmap, 0, 0, this.width, this.height);
                }
            },

            getFrameTimeMs: function(idx) {
                var frames = this.activeFrames();
                if (!frames) return 0;
                var t = 0;
                for (var i = 0; i < idx && i < frames.length; i++) t += frames[i].durationMs;
                return t;
            },

            seekToMs: async function(ms) {
                if (this._disposed) return;
                ms = Math.max(0, Math.min(this.durationMs, ms));
                this.currentTimeMs = ms;
                var frames = this.activeFrames();
                if (frames && frames.length) {
                    var acc = 0;
                    var idx = frames.length - 1;
                    for (var i = 0; i < frames.length; i++) {
                        acc += frames[i].durationMs;
                        if (ms < acc) { idx = i; break; }
                    }
                    this.currentFrameIndex = idx;
                    this._accumMs = 0;
                    this.drawCurrentToCanvas();
                    if (this._onFrameCallback) this._onFrameCallback(this);
                } else if (this.video) {
                    await seekVideoTo(this.video, ms / 1000);
                    this.drawCurrentToCanvas();
                    if (this._onFrameCallback) this._onFrameCallback(this);
                }
            },

            startLoop: function(onFrame) {
                this._onFrameCallback = onFrame || this._onFrameCallback;
                if (this._disposed) return;
                this.stopLoop();
                if (!this.playing) return;

                var self = this;
                if (this.isUsingFrames()) {
                    if (this.video && !this.video.paused) {
                        try { this.video.pause(); } catch (e) {}
                    }
                    this._lastTick = performance.now();
                    var tickFrames = function(now) {
                        if (self._disposed || !self.playing || !self.isUsingFrames()) {
                            self._rafId = null;
                            return;
                        }
                        var frames = self.activeFrames();
                        var dt = Math.min(250, Math.max(0, now - self._lastTick));
                        self._lastTick = now;
                        self._accumMs += dt;
                        var cur = frames[self.currentFrameIndex];
                        var changed = false;
                        while (cur && self._accumMs >= cur.durationMs) {
                            self._accumMs -= cur.durationMs;
                            self.currentFrameIndex = (self.currentFrameIndex + 1) % frames.length;
                            cur = frames[self.currentFrameIndex];
                            changed = true;
                        }
                        if (changed) {
                            self.currentTimeMs = self.getFrameTimeMs(self.currentFrameIndex);
                            self.drawCurrentToCanvas();
                            if (self._onFrameCallback) self._onFrameCallback(self);
                        }
                        self._rafId = requestAnimationFrame(tickFrames);
                    };
                    this._rafId = requestAnimationFrame(tickFrames);
                } else if (this.video) {
                    var p = this.video.play();
                    if (p && p.catch) p.catch(function() {});

                    if (typeof this.video.requestVideoFrameCallback === 'function') {
                        var onVideoFrame = function(now, metadata) {
                            if (self._disposed || !self.playing || self.isUsingFrames()) {
                                self._vfcId = null;
                                return;
                            }
                            if (metadata && typeof metadata.mediaTime === 'number') {
                                if (self._lastVfcMediaTime !== null) {
                                    var d = metadata.mediaTime - self._lastVfcMediaTime;
                                    if (d > 0.008 && d < 0.25 && self._fpsSamples.length < 20) {
                                        self._fpsSamples.push(1 / d);
                                        if (self._fpsSamples.length >= 6) {
                                            var sorted = self._fpsSamples.slice().sort(function(a, b) { return a - b; });
                                            var med = sorted[Math.floor(sorted.length / 2)];
                                            self.fps = Math.max(10, Math.min(60, Math.round(med)));
                                        }
                                    }
                                }
                                self._lastVfcMediaTime = metadata.mediaTime;
                            }
                            self.currentTimeMs = Math.round((self.video.currentTime || 0) * 1000);
                            self.drawCurrentToCanvas();
                            if (self._onFrameCallback) self._onFrameCallback(self);
                            self._vfcId = self.video.requestVideoFrameCallback(onVideoFrame);
                        };
                        this._vfcId = this.video.requestVideoFrameCallback(onVideoFrame);
                    } else {
                        var tickVideo = function() {
                            if (self._disposed || !self.playing || self.isUsingFrames()) {
                                self._rafId = null;
                                return;
                            }
                            var vt = self.video.currentTime;
                            if (vt !== self._lastVideoTime && self.video.readyState >= 2) {
                                self._lastVideoTime = vt;
                                self.currentTimeMs = Math.round(vt * 1000);
                                self.drawCurrentToCanvas();
                                if (self._onFrameCallback) self._onFrameCallback(self);
                            }
                            self._rafId = requestAnimationFrame(tickVideo);
                        };
                        this._rafId = requestAnimationFrame(tickVideo);
                    }
                }
            },

            stopLoop: function() {
                if (this._rafId) {
                    cancelAnimationFrame(this._rafId);
                    this._rafId = null;
                }
                if (this._vfcId && this.video && typeof this.video.cancelVideoFrameCallback === 'function') {
                    try { this.video.cancelVideoFrameCallback(this._vfcId); } catch (e) {}
                    this._vfcId = null;
                }
                if (this.video && !this.video.paused) {
                    try { this.video.pause(); } catch (e) {}
                }
            },

            pause: function(byUser) {
                if (byUser) this.userPaused = true;
                this.playing = false;
                this.stopLoop();
            },

            play: function(byUser) {
                if (this._disposed) return;
                if (byUser) this.userPaused = false;
                this.playing = true;
                this.startLoop();
            },

            setShowingCutout: function(showCutout) {
                var want = !!(showCutout && this.cutoutFrames && this.cutoutFrames.length);
                if (this.showingCutout === want) return;
                var wasPlaying = this.playing;
                this.stopLoop();
                this.showingCutout = want;
                this.drawCurrentToCanvas();
                if (this._onFrameCallback) this._onFrameCallback(this);
                if (wasPlaying) this.startLoop();
            },

            dispose: function() {
                this._disposed = true;
                this.playing = false;
                this.stopLoop();
                if (this.originalFrames) {
                    this.originalFrames.forEach(function(fr) {
                        if (fr.bitmap && fr.bitmap.close) fr.bitmap.close();
                    });
                    this.originalFrames = null;
                }
                if (this.cutoutFrames) {
                    this.cutoutFrames.forEach(function(fr) {
                        if (fr.bitmap && fr.bitmap.close) fr.bitmap.close();
                    });
                    this.cutoutFrames = null;
                }
                if (this.video) {
                    try {
                        this.video.pause();
                        this.video.removeAttribute('src');
                        this.video.load();
                    } catch (e) {}
                    this.video = null;
                }
                if (this.videoUrl) {
                    URL.revokeObjectURL(this.videoUrl);
                    this.videoUrl = null;
                }
                if (this.frameCanvas) {
                    this.frameCanvas.width = 1;
                    this.frameCanvas.height = 1;
                }
            }
        };

        // Draw initial frame immediately
        session.drawCurrentToCanvas();
        return session;
    }

    /**
     * Tries to load a File as animated media (GIF, animated WebP/APNG, or video).
     * Returns a MediaSession if animated/video, or null if it's a static image.
     */
    async function tryLoadAnimated(file) {
        if (!file) return null;
        if (isVideoFile(file)) {
            var vid = await decodeVideoFile(file);
            return createMediaSession(vid, file);
        }
        if (isAnimatedCandidate(file)) {
            var anim = await decodeAnimatedImage(file);
            if (anim) return createMediaSession(anim, file);
        }
        return null;
    }

    /**
     * Computes export frame schedule for a MediaSession.
     */
    function getExportSchedule(media) {
        if (media.isUsingFrames()) {
            var frames = media.activeFrames();
            return {
                mode: 'frames',
                count: frames.length,
                fps: media.fps || 15,
                frames: frames
            };
        }
        var fps = Math.max(10, Math.min(MAX_EXPORT_FPS, media.fps || DEFAULT_VIDEO_FPS));
        var durSec = Math.max(0.1, media.durationMs / 1000);
        var count = Math.max(1, Math.min(MAX_DECODE_FRAMES, Math.round(durSec * fps)));
        var stepMs = (durSec * 1000) / count;
        return {
            mode: 'video',
            count: count,
            fps: fps,
            stepMs: stepMs,
            durSec: durSec
        };
    }

    /**
     * Steps through every frame of `media` (pausing playback during export, restoring after),
     * calls `renderFrame()` for each frame, and encodes the result as a WebM with alpha.
     */
    async function exportSessionToWebm(media, opts) {
        var wasPlaying = media.playing;
        var savedTimeMs = media.currentTimeMs;
        var savedFrameIdx = media.currentFrameIndex;
        media.stopLoop();
        media.playing = false;

        try {
            var sched = getExportSchedule(media);
            var firstRender = null;

            // Prepare frame 0 to determine dimensions
            if (sched.mode === 'frames') {
                media.currentFrameIndex = 0;
                media.drawCurrentToCanvas();
            } else if (media.video) {
                await seekVideoTo(media.video, 0);
                media.drawCurrentToCanvas();
            }
            if (opts.onBeforeFrame) opts.onBeforeFrame();
            firstRender = opts.renderFrame(0);
            if (!firstRender || !firstRender.canvas) {
                throw new Error('Не удалось отрендерить кадр');
            }
            var outW = firstRender.canvas.width;
            var outH = firstRender.canvas.height;

            return await encodeFramesToWebm({
                width: outW,
                height: outH,
                frameCount: sched.count,
                fps: sched.fps,
                compression: opts.compression,
                hasAlpha: opts.hasAlpha !== false,
                onProgress: opts.onProgress,
                signal: opts.signal,
                getFrame: async function(idx) {
                    if (idx === 0 && firstRender) {
                        var fr0 = firstRender;
                        firstRender = null;
                        return {
                            canvas: fr0.canvas,
                            durationMs: sched.mode === 'frames'
                                ? sched.frames[0].durationMs
                                : sched.stepMs
                        };
                    }
                    if (sched.mode === 'frames') {
                        media.currentFrameIndex = idx;
                        media.drawCurrentToCanvas();
                    } else if (media.video) {
                        var tSec = (idx / sched.count) * sched.durSec;
                        await seekVideoTo(media.video, tSec);
                        media.drawCurrentToCanvas();
                    }
                    if (opts.onBeforeFrame) opts.onBeforeFrame();
                    var r = opts.renderFrame(idx);
                    return {
                        canvas: r.canvas,
                        durationMs: sched.mode === 'frames'
                            ? sched.frames[idx].durationMs
                            : sched.stepMs
                    };
                }
            });
        } finally {
            media.currentFrameIndex = savedFrameIdx;
            media.currentTimeMs = savedTimeMs;
            if (media.isUsingFrames()) {
                media.drawCurrentToCanvas();
            } else if (media.video) {
                await seekVideoTo(media.video, savedTimeMs / 1000);
                media.drawCurrentToCanvas();
            }
            if (opts.onBeforeFrame) opts.onBeforeFrame();
            if (wasPlaying && !media.userPaused && !media._disposed) {
                media.play(false);
            }
        }
    }

    /**
     * Removes background frame-by-frame via POST /process on a MediaSession
     * (or standalone file), returning Array<{ bitmap, durationMs }>.
     */
    async function removeBackgroundFrames(media, opts) {
        opts = opts || {};
        var signal = opts.signal || null;
        var edgeBlur = opts.edgeBlur !== undefined ? opts.edgeBlur : 1;
        var onProgress = opts.onProgress || null;

        var wasPlaying = media.playing;
        var savedTimeMs = media.currentTimeMs;
        var savedFrameIdx = media.currentFrameIndex;
        media.stopLoop();
        media.playing = false;

        // Build schedule strictly from ORIGINAL source
        var sched;
        if (media.originalFrames && media.originalFrames.length) {
            sched = {
                mode: 'frames',
                count: media.originalFrames.length,
                frames: media.originalFrames
            };
        } else {
            var fps = Math.max(10, Math.min(MAX_EXPORT_FPS, media.fps || DEFAULT_VIDEO_FPS));
            var durSec = Math.max(0.1, media.durationMs / 1000);
            var count = Math.max(1, Math.min(300, Math.round(durSec * fps)));
            sched = {
                mode: 'video',
                count: count,
                stepMs: (durSec * 1000) / count,
                durSec: durSec
            };
        }

        var tempCanvas = document.createElement('canvas');
        tempCanvas.width = media.width;
        tempCanvas.height = media.height;
        var tCtx = tempCanvas.getContext('2d');

        var cutoutFrames = [];
        try {
            for (var i = 0; i < sched.count; i++) {
                if (signal && signal.aborted) throw new DOMException('Aborted', 'AbortError');

                tCtx.fillStyle = '#ffffff';
                tCtx.fillRect(0, 0, tempCanvas.width, tempCanvas.height);

                var durMs;
                if (sched.mode === 'frames') {
                    var srcFr = sched.frames[i];
                    durMs = srcFr.durationMs;
                    tCtx.drawImage(srcFr.bitmap, 0, 0, tempCanvas.width, tempCanvas.height);
                } else {
                    durMs = sched.stepMs;
                    await seekVideoTo(media.video, (i / sched.count) * sched.durSec);
                    tCtx.drawImage(media.video, 0, 0, tempCanvas.width, tempCanvas.height);
                }

                var jpgBlob = await new Promise(function(resolve) {
                    tempCanvas.toBlob(resolve, 'image/jpeg', 0.92);
                });
                var fd = new FormData();
                fd.append('image', jpgBlob, 'frame.jpg');
                fd.append('format', 'png');
                fd.append('edge_blur', edgeBlur);

                var res = await fetch('/process', { method: 'POST', body: fd, signal: signal });
                if (!res.ok) {
                    var errData = await res.json().catch(function() { return {}; });
                    throw new Error(errData.error || 'Ошибка удаления фона');
                }
                var outBlob = await res.blob();
                var bmp = await createImageBitmap(outBlob, {
                    resizeWidth: media.width,
                    resizeHeight: media.height,
                    resizeQuality: 'high'
                });
                cutoutFrames.push({ bitmap: bmp, durationMs: durMs });
                if (onProgress) onProgress(i + 1, sched.count);
            }
            return cutoutFrames;
        } catch (err) {
            cutoutFrames.forEach(function(fr) { if (fr.bitmap && fr.bitmap.close) fr.bitmap.close(); });
            throw err;
        } finally {
            tempCanvas.width = 1;
            tempCanvas.height = 1;
            media.currentFrameIndex = savedFrameIdx;
            media.currentTimeMs = savedTimeMs;
            if (wasPlaying && !media.userPaused && !media._disposed) {
                media.play(false);
            }
        }
    }

    /**
     * Direct standalone conversion of a video/GIF file to WebM in the browser
     * (used by Converter fallback when FFmpeg is not installed, and by batch Remover).
     */
    async function convertFileToWebm(file, opts) {
        opts = opts || {};
        var media = await tryLoadAnimated(file);
        if (!media) throw new Error('Файл не является видео или анимацией');
        try {
            if (opts.removeBg) {
                var cutout = await removeBackgroundFrames(media, {
                    edgeBlur: opts.edgeBlur,
                    onProgress: opts.onProgress,
                    signal: opts.signal
                });
                media.cutoutFrames = cutout;
                media.showingCutout = true;
            }
            var hasAlpha = opts.removeBg || isGifFile(file) || getExt(file.name) === 'webm'
                || getExt(file.name) === 'webp' || getExt(file.name) === 'apng'
                || getExt(file.name) === 'png';
            return await exportSessionToWebm(media, {
                compression: opts.quality || 90,
                hasAlpha: hasAlpha,
                onProgress: opts.onEncodeProgress || null,
                signal: opts.signal,
                renderFrame: function() {
                    return { canvas: media.frameCanvas };
                }
            });
        } finally {
            media.dispose();
        }
    }

    // =========================================================================
    // Canvas Progress & Playback UI Helpers
    // =========================================================================

    var _activeAbort = null;

    function showProgressOverlay(title, done, total, abortCtrl) {
        var el = document.getElementById('mediaProgressOverlay');
        if (!el) return;
        _activeAbort = abortCtrl || null;
        var titleEl = document.getElementById('mediaProgressTitle');
        var fillEl = document.getElementById('mediaProgressFill');
        var textEl = document.getElementById('mediaProgressText');
        var cancelBtn = document.getElementById('mediaProgressCancel');
        if (titleEl) titleEl.textContent = typeof I18n !== 'undefined' ? I18n.t(title) : title;
        var pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
        if (fillEl) fillEl.style.width = pct + '%';
        if (textEl) textEl.textContent = total > 0 ? (done + ' / ' + total) : '';
        if (cancelBtn) {
            cancelBtn.style.display = abortCtrl ? '' : 'none';
            cancelBtn.onclick = function() {
                if (_activeAbort) _activeAbort.abort();
            };
        }
        el.hidden = false;
    }

    function hideProgressOverlay() {
        var el = document.getElementById('mediaProgressOverlay');
        if (el) el.hidden = true;
        _activeAbort = null;
    }

    function formatSec(ms) {
        return (Math.max(0, ms) / 1000).toFixed(1) + 's';
    }

    function updatePlayBar(media) {
        var bar = document.getElementById('mediaPlayBar');
        if (!bar) return;
        if (!media || media._disposed) {
            bar.hidden = true;
            return;
        }
        bar.hidden = false;
        var btn = document.getElementById('mediaPlayPauseBtn');
        if (btn) {
            btn.classList.toggle('paused', !media.playing);
            btn.dataset.tooltip = typeof I18n !== 'undefined'
                ? I18n.t(media.playing ? 'Пауза' : 'Воспроизвести')
                : (media.playing ? 'Пауза' : 'Воспроизвести');
            btn.setAttribute('aria-label', btn.dataset.tooltip);
        }
        var range = document.getElementById('mediaSeekSlider');
        if (range && !range._scrubbing) {
            var ratio = media.durationMs > 0 ? Math.min(1000, Math.round((media.currentTimeMs / media.durationMs) * 1000)) : 0;
            range.value = ratio;
            range.style.setProperty('--p', (ratio / 10) + '%');
        }
        var timeEl = document.getElementById('mediaTimeLabel');
        if (timeEl) {
            timeEl.textContent = formatSec(media.currentTimeMs) + ' / ' + formatSec(media.durationMs);
        }
    }

    function initPlayBar() {
        var btn = document.getElementById('mediaPlayPauseBtn');
        var range = document.getElementById('mediaSeekSlider');
        if (btn && !btn.dataset.bound) {
            btn.dataset.bound = '1';
            btn.onclick = function() {
                if (!state.media) return;
                if (state.media.playing) state.media.pause(true);
                else state.media.play(true);
                updatePlayBar(state.media);
            };
        }
        if (range && !range.dataset.bound) {
            range.dataset.bound = '1';
            var wasPlayingBeforeScrub = false;
            range.onpointerdown = function() {
                if (!state.media) return;
                range._scrubbing = true;
                wasPlayingBeforeScrub = state.media.playing;
                if (wasPlayingBeforeScrub) state.media.pause(false);
            };
            range.oninput = function() {
                if (!state.media) return;
                var ms = (parseInt(range.value) / 1000) * state.media.durationMs;
                range.style.setProperty('--p', (parseInt(range.value) / 10) + '%');
                state.media.seekToMs(ms);
                var timeEl = document.getElementById('mediaTimeLabel');
                if (timeEl) timeEl.textContent = formatSec(ms) + ' / ' + formatSec(state.media.durationMs);
            };
            var endScrub = function() {
                if (!range._scrubbing) return;
                range._scrubbing = false;
                if (state.media && wasPlayingBeforeScrub && !state.media.userPaused) {
                    state.media.play(false);
                    updatePlayBar(state.media);
                }
            };
            range.onpointerup = endScrub;
            range.onchange = endScrub;
        }
    }

    window.TokenMedia = {
        VIDEO_EXTS: VIDEO_EXTS,
        IMAGE_EXTS: IMAGE_EXTS,
        isVideoFile: isVideoFile,
        isGifFile: isGifFile,
        isAnimatedCandidate: isAnimatedCandidate,
        isSupportedFile: isSupportedFile,
        guessMime: guessMime,
        muxWebm: muxWebm,
        encodeFramesToWebm: encodeFramesToWebm,
        tryLoadAnimated: tryLoadAnimated,
        getExportSchedule: getExportSchedule,
        exportSessionToWebm: exportSessionToWebm,
        removeBackgroundFrames: removeBackgroundFrames,
        convertFileToWebm: convertFileToWebm,
        seekVideoTo: seekVideoTo,
        showProgressOverlay: showProgressOverlay,
        hideProgressOverlay: hideProgressOverlay,
        updatePlayBar: updatePlayBar,
        initPlayBar: initPlayBar
    };
})();
