export const PROFILE_STORE_VERSION = 5;

export function emptyProfileStore() {
  return { version: PROFILE_STORE_VERSION, cameras: {} };
}

export function normalizeProfileStore(value) {
  if (!value?.cameras || typeof value.cameras !== 'object' || Array.isArray(value.cameras)) {
    return emptyProfileStore();
  }
  if (![3, 4, PROFILE_STORE_VERSION].includes(value.version)) return emptyProfileStore();
  const normalized = emptyProfileStore();
  for (const [cameraId, camera] of Object.entries(value.cameras).slice(0, 64)) {
    if (!cameraId || cameraId.length > 2048) continue;
    const profiles = camera?.profiles;
    if (!profiles || typeof profiles !== 'object' || Array.isArray(profiles)) continue;
    const normalizedProfiles = {};
    for (const [rawName, profile] of Object.entries(profiles).slice(0, 100)) {
      const name = rawName.trim().slice(0, 80);
      if (!name || Object.hasOwn(normalizedProfiles, name)) continue;
      let normalizedProfile;
      if (Array.isArray(profile)) {
        normalizedProfile = {
          controls: normalizeSavedControls(profile),
          filterGraph: defaultFilterGraph()
        };
      } else if (profile && typeof profile === 'object') {
        normalizedProfile = {
          controls: normalizeSavedControls(profile.controls),
          filterGraph: profile.filterGraph
            ? normalizeFilterGraph(profile.filterGraph)
            : legacyFiltersToGraph(profile.filters)
        };
      }
      if (normalizedProfile) defineOwn(normalizedProfiles, name, normalizedProfile);
    }
    defineOwn(normalized.cameras, cameraId, { profiles: normalizedProfiles });
  }
  return normalized;
}

export function profilesForCamera(store, cameraId, create = false) {
  if (!cameraId) return {};
  if (!Object.hasOwn(store.cameras, cameraId) && create) {
    defineOwn(store.cameras, cameraId, { profiles: {} });
  }
  const profiles = Object.hasOwn(store.cameras, cameraId)
    ? store.cameras[cameraId]?.profiles
    : null;
  return profiles && typeof profiles === 'object' && !Array.isArray(profiles) ? profiles : {};
}

function defineOwn(target, key, value) {
  Object.defineProperty(target, key, {
    configurable: true,
    enumerable: true,
    value,
    writable: true
  });
}

function normalizeSavedControls(value) {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 64).flatMap((control) => {
    const id = typeof control?.id === 'string' ? control.id.trim().slice(0, 128) : '';
    const numericValue = Number(control?.value);
    if (!id || !Number.isFinite(numericValue)) return [];
    return [{ id, value: numericValue, automatic: Boolean(control?.automatic) }];
  });
}

export function normalizeNotifications(value, maximum = 100) {
  if (!Array.isArray(value) || !Number.isInteger(maximum) || maximum < 0) return [];
  return value.slice(0, maximum).flatMap((item, index) => {
    const timestamp = Number(item?.timestamp);
    const message = typeof item?.message === 'string' ? item.message.slice(0, 4096) : '';
    if (!Number.isFinite(timestamp) || Number.isNaN(new Date(timestamp).valueOf()) || !message) {
      return [];
    }
    return [{
      id: typeof item.id === 'string' && item.id ? item.id.slice(0, 128) : `${timestamp}-${index}`,
      timestamp,
      title: typeof item.title === 'string' && item.title ? item.title.slice(0, 256) : 'Error',
      message,
      source: typeof item.source === 'string' ? item.source.slice(0, 128) : 'aplicación',
      code: typeof item.code === 'string' ? item.code.slice(0, 64) : null,
      read: Boolean(item.read)
    }];
  });
}

export function clampControlValue(control, value) {
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return control.defaultValue;
  const clamped = Math.min(control.maximum, Math.max(control.minimum, numericValue));
  const step = Math.max(1, Math.abs(Number(control.step) || 1));
  return control.minimum + Math.round((clamped - control.minimum) / step) * step;
}

export function isCurrentCameraRequest(requestCameraId, requestRevision, currentCameraId, currentRevision) {
  return Boolean(requestCameraId)
    && requestCameraId === currentCameraId
    && requestRevision === currentRevision;
}

export function summarizeVideoFormats(formats) {
  if (!Array.isArray(formats) || formats.length === 0) return 'Modos nativos no disponibles';
  const valid = formats.filter((format) => Number(format?.width) > 0 && Number(format?.height) > 0);
  if (valid.length === 0) return 'Modos nativos no disponibles';
  const best = valid.reduce((current, candidate) => {
    const currentPixels = Number(current.width) * Number(current.height);
    const candidatePixels = Number(candidate.width) * Number(candidate.height);
    if (candidatePixels !== currentPixels) return candidatePixels > currentPixels ? candidate : current;
    const currentFps = Number(current.fpsNumerator) / Math.max(1, Number(current.fpsDenominator));
    const candidateFps = Number(candidate.fpsNumerator) / Math.max(1, Number(candidate.fpsDenominator));
    return candidateFps > currentFps ? candidate : current;
  });
  const fps = Number(best.fpsNumerator) / Math.max(1, Number(best.fpsDenominator));
  const fpsLabel = Number.isInteger(fps) ? String(fps) : fps.toFixed(2);
  return `${valid.length} modos · hasta ${best.width}×${best.height} · ${fpsLabel} FPS`;
}

export function videoFormatKey(format) {
  const width = Number(format?.width);
  const height = Number(format?.height);
  const numerator = Number(format?.fpsNumerator);
  const denominator = Number(format?.fpsDenominator);
  const pixelFormat = String(format?.pixelFormat || '');
  if (![width, height, numerator, denominator].every(Number.isInteger)
    || width <= 0 || height <= 0 || numerator <= 0 || denominator <= 0 || !pixelFormat) return '';
  return `${width}x${height}@${numerator}/${denominator}|${pixelFormat}|${String(format?.subtypeGuid || '')}`;
}

export function formatVideoMode(format) {
  if (!videoFormatKey(format)) return 'Modo no válido';
  const fps = Number(format.fpsNumerator) / Number(format.fpsDenominator);
  const fpsLabel = Number.isInteger(fps) ? String(fps) : fps.toFixed(2);
  return `${format.width}×${format.height} · ${fpsLabel} FPS · ${format.pixelFormat}`;
}

export function sortVideoFormats(formats) {
  if (!Array.isArray(formats)) return [];
  const unique = new Map();
  for (const format of formats) {
    const key = videoFormatKey(format);
    if (key && !unique.has(key)) unique.set(key, format);
  }
  const pixelPriority = ['NV12', 'YUY2', 'MJPEG', 'H264', 'BGRA'];
  return [...unique.values()].sort((left, right) => {
    const pixels = right.width * right.height - left.width * left.height;
    if (pixels !== 0) return pixels;
    const leftFps = left.fpsNumerator / left.fpsDenominator;
    const rightFps = right.fpsNumerator / right.fpsDenominator;
    if (leftFps !== rightFps) return rightFps - leftFps;
    const leftPriority = pixelPriority.indexOf(left.pixelFormat);
    const rightPriority = pixelPriority.indexOf(right.pixelFormat);
    return (leftPriority < 0 ? pixelPriority.length : leftPriority)
      - (rightPriority < 0 ? pixelPriority.length : rightPriority);
  });
}

export function measuredProbeFps(result) {
  const frames = Number(result?.receivedFrames);
  const firstTimestamp = Number(result?.firstTimestamp100ns);
  const lastTimestamp = Number(result?.lastTimestamp100ns);
  if (frames > 1 && Number.isFinite(firstTimestamp) && Number.isFinite(lastTimestamp) && lastTimestamp > firstTimestamp) {
    return (frames - 1) * 10_000_000 / (lastTimestamp - firstTimestamp);
  }
  const elapsedMillis = Number(result?.elapsedMillis);
  return frames > 0 && elapsedMillis > 0 ? frames * 1000 / elapsedMillis : null;
}

export function defaultFilterGraph() {
  return { nodes: [] };
}

export function normalizeFilterGraph(value) {
  if (!value || typeof value !== 'object' || !Array.isArray(value.nodes)) return defaultFilterGraph();
  const ids = new Set();
  const nodes = [];
  for (const candidate of value.nodes.slice(0, 64)) {
    const node = normalizeFilterNode(candidate);
    if (!node) continue;
    let id = node.id;
    let suffix = 2;
    while (ids.has(id)) id = `${node.id.slice(0, 58)}-${suffix++}`;
    node.id = id;
    ids.add(id);
    nodes.push(node);
  }
  return { nodes };
}

export function legacyFiltersToGraph(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return defaultFilterGraph();
  const number = (candidate, minimum, maximum, defaultValue) => {
    const parsed = Number(candidate);
    return Number.isFinite(parsed) ? Math.min(maximum, Math.max(minimum, parsed)) : defaultValue;
  };
  const nodes = [];
  const add = (type, fields) => nodes.push({ id: `migrated-${type}-${nodes.length + 1}`, enabled: true, type, ...fields });
  const brightness = number(value.brightness, -0.5, 0.5, 0);
  const contrast = number(value.contrast, 0, 2, 1);
  const saturation = number(value.saturation, 0, 2, 1);
  const gamma = number(value.gamma, 0.25, 2.5, 1);
  const temperature = number(value.temperature, -0.5, 0.5, 0);
  const tint = number(value.tint, -0.5, 0.5, 0);
  if (value.lens?.enabled) add('lensCorrection', {
    k1: number(value.lens.k1, -0.5, 0.5, 0),
    k2: number(value.lens.k2, -0.25, 0.25, 0),
    k3: number(value.lens.k3, -0.1, 0.1, 0),
    p1: number(value.lens.p1, -0.05, 0.05, 0),
    p2: number(value.lens.p2, -0.05, 0.05, 0),
    scale: number(value.lens.scale, -0.25, 0.5, 0)
  });
  if (temperature !== 0) add('temperature', { amount: temperature });
  if (tint !== 0) add('tint', { amount: tint });
  if (contrast !== 1) add('contrast', { amount: contrast });
  if (brightness !== 0) add('brightness', { amount: brightness });
  if (saturation !== 1) add('saturation', { amount: saturation });
  if (gamma !== 1) add('gamma', { amount: gamma });
  if (value.flipHorizontal || value.flipVertical) add('flip', {
    horizontal: Boolean(value.flipHorizontal),
    vertical: Boolean(value.flipVertical)
  });
  return { nodes };
}

function normalizeFilterNode(value) {
  if (!value || typeof value !== 'object') return null;
  const id = String(value.id || '').trim().replace(/[^a-zA-Z0-9._-]/g, '-').slice(0, 64);
  if (!id) return null;
  const enabled = value.enabled !== false;
  const label = typeof value.label === 'string' && value.label.trim()
    ? value.label.trim().slice(0, 80)
    : undefined;
  const number = (candidate, minimum, maximum, defaultValue) => {
    const parsed = Number(candidate);
    return Number.isFinite(parsed) ? Math.min(maximum, Math.max(minimum, parsed)) : defaultValue;
  };
  const base = { id, enabled, ...(label ? { label } : {}) };
  switch (value.type) {
    case 'brightness':
    case 'temperature':
    case 'tint':
      return { ...base, type: value.type, amount: number(value.amount, -0.5, 0.5, 0) };
    case 'contrast':
    case 'saturation':
      return { ...base, type: value.type, amount: number(value.amount, 0, 2, 1) };
    case 'gamma':
      return { ...base, type: value.type, amount: number(value.amount, 0.25, 2.5, 1) };
    case 'flip':
      return { ...base, type: 'flip', horizontal: Boolean(value.horizontal), vertical: Boolean(value.vertical) };
    case 'lensCorrection':
      return {
        ...base,
        type: 'lensCorrection',
        k1: number(value.k1, -0.5, 0.5, 0),
        k2: number(value.k2, -0.25, 0.25, 0),
        k3: number(value.k3, -0.1, 0.1, 0),
        p1: number(value.p1, -0.05, 0.05, 0),
        p2: number(value.p2, -0.05, 0.05, 0),
        scale: number(value.scale, -0.25, 0.5, 0)
      };
    case 'lut3d':
      return {
        ...base,
        type: 'lut3d',
        ...(typeof value.assetId === 'string' && value.assetId ? { assetId: value.assetId.slice(0, 64) } : {}),
        ...(typeof value.name === 'string' && value.name ? { name: value.name.slice(0, 255) } : {}),
        strength: number(value.strength, 0, 1, 1)
      };
    case 'plugin': {
      const pluginId = String(value.pluginId || '').slice(0, 64);
      if (!pluginId) return null;
      const parameters = {};
      if (value.parameters && typeof value.parameters === 'object' && !Array.isArray(value.parameters)) {
        for (const [key, candidate] of Object.entries(value.parameters).slice(0, 32)) {
          const parsed = Number(candidate);
          if (Number.isFinite(parsed)) parameters[key.slice(0, 64)] = parsed;
        }
      }
      return { ...base, type: 'plugin', pluginId, parameters };
    }
    default:
      return null;
  }
}

export function redactDiagnosticContext(value, key = '', depth = 0) {
  if (depth >= 8) return '[depth-limited]';
  if (/^(cameraId|deviceId|devicePath|path|id|cube|manifestJson)$/i.test(key)) return '[redacted]';
  if (typeof value === 'string') return value.length > 4096 ? `${value.slice(0, 4096)}…[truncated]` : value;
  if (Array.isArray(value)) return value.slice(0, 100).map((item) => redactDiagnosticContext(item, '', depth + 1));
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).slice(0, 100)
      .map(([childKey, childValue]) => [childKey, redactDiagnosticContext(childValue, childKey, depth + 1)]));
  }
  return value;
}
