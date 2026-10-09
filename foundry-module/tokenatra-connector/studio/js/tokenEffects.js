function createDropShadow(sourceCanvas, lowRes) {
    // Интерактивная тень (кэш) считается в пониженном разрешении: блюр
    // 6144² в разы дешевле, на экране неотличимо. Экспорт (renderForSave)
    // всегда просит полное разрешение.
    const size = sourceCanvas.width;
    const outSize = (lowRes && size > 1024) ? Math.max(1024, Math.round(size / 2)) : size;

    const shadowCanvas = document.createElement('canvas');
    shadowCanvas.width = outSize;
    shadowCanvas.height = outSize;
    const shadowCtx = shadowCanvas.getContext('2d');

    const scale = outSize / 1024;
    const s = state.dropShadowSettings;
    const angleRad = s.angle * Math.PI / 180;
    const distance = s.distance * scale;
    const offsetX = Math.cos(angleRad) * distance;
    const offsetY = -Math.sin(angleRad) * distance;
    const blurRadius = s.blur * scale;
    const opacity = s.opacity;

    const tempCanvas = document.createElement('canvas');
    tempCanvas.width = outSize;
    tempCanvas.height = outSize;
    const tempCtx = tempCanvas.getContext('2d');

    tempCtx.drawImage(sourceCanvas, offsetX, offsetY, outSize, outSize);

    // Черная заливка по форме источника (source-in сохраняет альфу, умножая на opacity)
    tempCtx.globalCompositeOperation = 'source-in';
    tempCtx.fillStyle = 'rgba(0, 0, 0, ' + opacity + ')';
    tempCtx.fillRect(0, 0, outSize, outSize);
    tempCtx.globalCompositeOperation = 'source-over';

    shadowCtx.filter = `blur(${blurRadius}px)`;
    shadowCtx.drawImage(tempCanvas, 0, 0);
    shadowCtx.filter = 'none';

    return shadowCanvas;
}

function colorCorrectionFilter() {
    const s = state.colorCorrectionSettings;
    const sat = (s.saturation || 0) / 100;
    const light = (s.lightness || 0) / 100;
    const parts = [];
    if (sat !== 0) parts.push(`saturate(${1 + sat})`);
    if (light !== 0) parts.push(`brightness(${1 + light})`);
    return parts.join(' ');
}
