import test from 'node:test';
import assert from 'node:assert/strict';
import {
  PROFILE_STORE_VERSION,
  emptyProfileStore,
  normalizeProfileStore,
  profilesForCamera,
  clampControlValue,
  defaultFilterGraph,
  isCurrentCameraRequest,
  measuredProbeFps,
  normalizeFilterGraph,
  normalizeNotifications,
  redactDiagnosticContext,
  summarizeVideoFormats,
  videoFormatKey,
  formatVideoMode,
  sortVideoFormats
} from '../ui/lib.js';

test('normaliza almacenes de perfiles inválidos', () => {
  assert.deepEqual(normalizeProfileStore(null), emptyProfileStore());
  assert.deepEqual(normalizeProfileStore({ version: 2, cameras: {} }), emptyProfileStore());
  assert.equal(emptyProfileStore().version, PROFILE_STORE_VERSION);
});

test('migra perfiles v3 y agrega un grafo digital vacío', () => {
  const migrated = normalizeProfileStore({
    version: 3,
    cameras: {
      camera: { profiles: { Studio: [{ id: 'brightness', value: 12 }] } }
    }
  });
  assert.equal(migrated.version, PROFILE_STORE_VERSION);
  assert.deepEqual(migrated.cameras.camera.profiles.Studio.controls, [
    { id: 'brightness', value: 12, automatic: false }
  ]);
  assert.deepEqual(migrated.cameras.camera.profiles.Studio.filterGraph, defaultFilterGraph());
});

test('separa perfiles por identificador de cámara', () => {
  const store = emptyProfileStore();
  profilesForCamera(store, 'camera-a', true).Day = [{ id: 'exposure', value: -5 }];
  profilesForCamera(store, 'camera-b', true).Night = [{ id: 'gain', value: 10 }];
  assert.deepEqual(Object.keys(profilesForCamera(store, 'camera-a')), ['Day']);
  assert.deepEqual(Object.keys(profilesForCamera(store, 'camera-b')), ['Night']);
});

test('normaliza perfiles actuales y evita claves heredadas peligrosas', () => {
  const normalized = normalizeProfileStore({
    version: PROFILE_STORE_VERSION,
    cameras: {
      camera: {
        profiles: {
          '  Studio  ': {
            controls: [
              { id: 'exposure', value: '-5', automatic: true },
              { id: '', value: 3 }
            ],
            filterGraph: { nodes: [{ id: 'bright', type: 'brightness', amount: 9 }] }
          }
        }
      }
    }
  });

  assert.deepEqual(normalized.cameras.camera.profiles.Studio.controls, [
    { id: 'exposure', value: -5, automatic: true }
  ]);
  assert.equal(normalized.cameras.camera.profiles.Studio.filterGraph.nodes[0].amount, 0.5);

  const store = emptyProfileStore();
  profilesForCamera(store, '__proto__', true).Safe = { controls: [] };
  assert.equal(Object.getPrototypeOf(store.cameras), Object.prototype);
  assert.equal(Object.hasOwn(store.cameras, '__proto__'), true);
});

test('descarta notificaciones persistidas malformadas', () => {
  assert.deepEqual(normalizeNotifications([
    { timestamp: 'invalid', message: 'bad' },
    { timestamp: 1234, message: 'valid', title: 'Camera', read: 1 }
  ]), [{
    id: '1234-1',
    timestamp: 1234,
    title: 'Camera',
    message: 'valid',
    source: 'aplicación',
    code: null,
    read: true
  }]);
});

test('limita y alinea valores al rango del controlador', () => {
  const control = { minimum: 10, maximum: 50, step: 5, defaultValue: 20 };
  assert.equal(clampControlValue(control, 1), 10);
  assert.equal(clampControlValue(control, 53), 50);
  assert.equal(clampControlValue(control, 23), 25);
  assert.equal(clampControlValue(control, 'invalid'), 20);
});

test('rechaza respuestas pertenecientes a otra selección', () => {
  assert.equal(isCurrentCameraRequest('a', 3, 'a', 3), true);
  assert.equal(isCurrentCameraRequest('a', 2, 'a', 3), false);
  assert.equal(isCurrentCameraRequest('a', 3, 'b', 3), false);
});

test('redacta identificadores sensibles del diagnóstico', () => {
  assert.deepEqual(redactDiagnosticContext({
    cameraId: '@device:pnp:secret',
    options: { devicePath: 'secret-path', width: 1280 },
    cube: 'LUT_3D_SIZE 65\n...',
    manifestJson: '{"private":"metadata"}',
    property: 4
  }), {
    cameraId: '[redacted]',
    options: { devicePath: '[redacted]', width: 1280 },
    cube: '[redacted]',
    manifestJson: '[redacted]',
    property: 4
  });
});

test('resume modos nativos por resolución y FPS máximos', () => {
  assert.equal(summarizeVideoFormats([]), 'Modos nativos no disponibles');
  assert.equal(summarizeVideoFormats([
    { width: 1280, height: 720, fpsNumerator: 60, fpsDenominator: 1 },
    { width: 1920, height: 1080, fpsNumerator: 30, fpsDenominator: 1 },
    { width: 1920, height: 1080, fpsNumerator: 60, fpsDenominator: 1 }
  ]), '3 modos · hasta 1920×1080 · 60 FPS');
});

test('mide FPS usando timestamps de cámara y no el tiempo de inicialización', () => {
  assert.equal(measuredProbeFps({
    receivedFrames: 30,
    firstTimestamp100ns: 10_000_000,
    lastTimestamp100ns: 20_000_000,
    elapsedMillis: 5000
  }), 29);
  assert.equal(measuredProbeFps({ receivedFrames: 30, elapsedMillis: 1000 }), 30);
  assert.equal(measuredProbeFps({ receivedFrames: 0, elapsedMillis: 0 }), null);
});

test('identifica, etiqueta y ordena todos los modos nativos', () => {
  const formats = [
    { width: 640, height: 360, fpsNumerator: 30, fpsDenominator: 1, pixelFormat: 'NV12' },
    { width: 1920, height: 1080, fpsNumerator: 30000, fpsDenominator: 1001, pixelFormat: 'MJPEG' },
    { width: 1280, height: 720, fpsNumerator: 60, fpsDenominator: 1, pixelFormat: 'YUY2' }
  ];
  assert.equal(videoFormatKey(formats[0]), '640x360@30/1|NV12|');
  assert.equal(formatVideoMode(formats[1]), '1920×1080 · 29.97 FPS · MJPEG');
  assert.deepEqual(sortVideoFormats([...formats, formats[0]]), [formats[1], formats[2], formats[0]]);
});

test('normaliza, limita y conserva el orden del grafo de filtros', () => {
  const normalized = normalizeFilterGraph({ nodes: [
    { id: 'bright', enabled: true, type: 'brightness', amount: 8 },
    { id: 'contrast', enabled: false, type: 'contrast', amount: '1.5' },
    { id: 'lens', enabled: true, type: 'lensCorrection', k1: -8, scale: 2 },
    { id: 'bad type!', type: 'unknown' }
  ] });
  assert.deepEqual(normalized.nodes.map((node) => node.type), [
    'brightness', 'contrast', 'lensCorrection'
  ]);
  assert.equal(normalized.nodes[0].amount, 0.5);
  assert.equal(normalized.nodes[1].amount, 1.5);
  assert.equal(normalized.nodes[1].enabled, false);
  assert.equal(normalized.nodes[2].k1, -0.5);
  assert.equal(normalized.nodes[2].scale, 0.5);
  assert.deepEqual(normalizeFilterGraph(null), defaultFilterGraph());
});

test('migra filtros planos v4 a nodos ordenados sin agregar identidades', () => {
  const migrated = normalizeProfileStore({
    version: 4,
    cameras: {
      camera: { profiles: { Studio: {
        controls: [],
        filters: { brightness: 0.2, contrast: 1, flipHorizontal: true }
      } } }
    }
  });
  assert.deepEqual(
    migrated.cameras.camera.profiles.Studio.filterGraph.nodes.map((node) => node.type),
    ['brightness', 'flip']
  );
});
