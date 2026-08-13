import {
  emptyProfileStore,
  defaultFilterGraph,
  normalizeProfileStore,
  normalizeNotifications,
  normalizeFilterGraph,
  profilesForCamera,
  clampControlValue,
  isCurrentCameraRequest,
  measuredProbeFps,
  redactDiagnosticContext,
  summarizeVideoFormats,
  videoFormatKey,
  formatVideoMode,
  sortVideoFormats
} from './lib.js';

let cameras = [];
let nativeCameras = [];
let nativeFormats = [];
let controls = [];
let selectedCameraId = '';
let cameraRevision = 0;
let previewState = 'stopped';
let virtualOutputState = 'stopped';
let virtualCameraInstalled = false;
let virtualCameraSupported = false;
let virtualOutputHealthCheckInFlight = false;
let previewTimer = null;
let previewObjectUrl = null;
let activePreviewFormat = null;
let controlOperationQueue = Promise.resolve();
let processingOperationQueue = Promise.resolve();
let nativeProbeRunning = false;
let filterGraph = defaultFilterGraph();
let filterPlugins = [];
let processingTimer = null;
let filterSequence = 0;
let notifications = [];
let notificationToastTimer = null;
let filterCatalogReturnFocus = null;
let profileDialogReturnFocus = null;

const scheduledControls = new Map();
const profileStorageKey = 'camera-tuner-profiles-v5';
const legacyProfileStorageKeys = ['camera-tuner-profiles-v4', 'camera-tuner-profiles-v3', 'control-webcam-profiles-v3'];
const themeStorageKey = 'camera-tuner-theme';
const legacyThemeStorageKey = 'control-webcam-theme';
const captureFormatStorageKey = 'camera-tuner-capture-formats-v1';
const notificationStorageKey = 'camera-tuner-notifications-v1';
const panelStorageKey = 'camera-tuner-panels-v1';
const inspectorWidthStorageKey = 'camera-tuner-inspector-width-v1';
const maxNotifications = 100;

const ui = {
  controls: document.querySelector('#control-list'),
  theme: document.querySelector('#theme-toggle'),
  camera: document.querySelector('#camera-select'),
  profile: document.querySelector('#profile-select'),
  state: document.querySelector('#camera-state'),
  refreshCameras: document.querySelector('#refresh-cameras'),
  saveProfile: document.querySelector('#save-profile'),
  deleteProfile: document.querySelector('#delete-profile'),
  restoreDefaults: document.querySelector('#restore-defaults'),
  driverProperties: document.querySelector('#driver-properties'),
  captureFormat: document.querySelector('#capture-format'),
  addFilter: document.querySelector('#add-filter'),
  filterList: document.querySelector('#filter-list'),
  filterTemplate: document.querySelector('#filter-template'),
  filterCatalog: document.querySelector('#filter-catalog'),
  filterCatalogNative: document.querySelector('#filter-catalog-native'),
  filterCatalogPlugins: document.querySelector('#filter-catalog-plugins'),
  filterPluginsEmpty: document.querySelector('#filter-plugins-empty'),
  filterPluginFile: document.querySelector('#filter-plugin-file'),
  previewToggle: document.querySelector('#preview-toggle'),
  previewEmpty: document.querySelector('#preview-empty'),
  previewImage: document.querySelector('#preview-image'),
  previewStage: document.querySelector('#preview-stage'),
  previewStatus: document.querySelector('#preview-status'),
  previewMode: document.querySelector('#preview-mode'),
  nativeFormatSummary: document.querySelector('#native-format-summary'),
  probeNativeBackend: document.querySelector('#probe-native-backend'),
  virtualCameraState: document.querySelector('#virtual-camera-state'),
  outputQuality: document.querySelector('#output-quality'),
  installVirtualCamera: document.querySelector('#install-virtual-camera'),
  removeVirtualCamera: document.querySelector('#remove-virtual-camera'),
  virtualOutputToggle: document.querySelector('#virtual-output-toggle'),
  openDiagnostics: document.querySelector('#open-diagnostics'),
  notificationToggle: document.querySelector('#notification-toggle'),
  notificationCount: document.querySelector('#notification-count'),
  notificationPanel: document.querySelector('#notification-panel'),
  notificationList: document.querySelector('#notification-list'),
  notificationToast: document.querySelector('#notification-toast'),
  clearNotifications: document.querySelector('#clear-notifications'),
  notificationOpenDiagnostics: document.querySelector('#notification-open-diagnostics'),
  profileDialog: document.querySelector('#profile-dialog'),
  profileForm: document.querySelector('#profile-form'),
  profileName: document.querySelector('#profile-name'),
  profileWarning: document.querySelector('#profile-warning'),
  profileSubmit: document.querySelector('#profile-submit'),
  profileCancel: document.querySelector('#profile-cancel'),
  profileDialogClose: document.querySelector('#profile-dialog-close'),
  confirmDialog: document.querySelector('#confirm-dialog'),
  confirmTitle: document.querySelector('#confirm-title'),
  confirmMessage: document.querySelector('#confirm-message'),
  confirmCancel: document.querySelector('#confirm-cancel'),
  confirmSubmit: document.querySelector('#confirm-submit'),
  workspace: document.querySelector('#workspace'),
  workspaceSplitter: document.querySelector('#workspace-splitter')
};

function hasTauriRuntime() {
  return Boolean(window.__TAURI__?.core?.invoke);
}

function readStorage(key) {
  try {
    return localStorage.getItem(key);
  } catch (error) {
    console.warn(`No se pudo leer el almacenamiento local (${key}).`, error);
    return null;
  }
}

function writeStorage(key, value) {
  try {
    localStorage.setItem(key, value);
    return true;
  } catch (error) {
    console.warn(`No se pudo escribir el almacenamiento local (${key}).`, error);
    return false;
  }
}

function messageFrom(error, fallback = 'Ocurrió un error inesperado') {
  if (typeof error === 'string' && error.trim()) return error;
  if (error instanceof Error && error.message) return error.message;
  return error?.toString?.() || fallback;
}

function commandLabel(command) {
  const labels = {
    get_preview_frame: 'Vista previa',
    start_preview: 'Vista previa',
    stop_preview: 'Vista previa',
    list_cameras: 'Detección de cámaras',
    list_native_cameras: 'Detección de cámaras',
    list_native_formats: 'Formatos de cámara',
    probe_media_frame_reader: 'Diagnóstico de cámara',
    get_controls: 'Controles de cámara',
    set_control: 'Controles de cámara',
    open_driver_property_page: 'Panel del fabricante',
    get_filter_graph: 'Filtros de software',
    set_filter_graph: 'Filtros de software',
    set_filter_lut_asset: 'LUT 3D',
    list_filter_plugins: 'Plugins',
    install_filter_plugin: 'Plugins',
    get_virtual_camera_status: 'Cámara virtual',
    install_virtual_camera: 'Cámara virtual',
    remove_virtual_camera: 'Cámara virtual',
    start_virtual_output: 'Salida virtual',
    stop_virtual_output: 'Salida virtual',
    get_virtual_output_running: 'Salida virtual',
    open_diagnostics_folder: 'Registros de diagnóstico'
  };
  return labels[command] || 'Operación';
}

function friendlyNotificationMessage(message) {
  if (/invalid frame exchange magic/i.test(message)) {
    return 'La vista previa recibió un fotograma incompatible. Detén la vista previa y vuelve a iniciarla.';
  }
  if (/MediaFrameReader failed to start with status 3/i.test(message)) {
    return 'No fue posible iniciar la captura con este modo. Prueba otro formato de cámara y vuelve a iniciar la vista previa.';
  }
  if (/class factory cannot supply requested class|classfactory no puede suministrar/i.test(message)) {
    return 'El componente de cámara virtual no está registrado correctamente. Vuelve a instalar la cámara virtual.';
  }
  return message;
}

function friendlyNotificationTitle(item) {
  const title = String(item?.title || 'Error');
  if (/^[a-z][a-z0-9_]+ falló$/i.test(title)) {
    return `${commandLabel(item?.source)}: error`;
  }
  return title;
}

function friendlyNotificationSource(source) {
  const value = String(source || 'aplicación');
  const label = commandLabel(value);
  if (label !== 'Operación') return label;
  return value === 'interfaz' ? 'Aplicación' : value;
}

async function invoke(command, args = {}) {
  if (!hasTauriRuntime()) throw new Error('La interfaz debe ejecutarse dentro de la aplicación Tauri.');
  const started = performance.now();
  const sampled = command === 'get_preview_frame' || command === 'get_virtual_output_running';
  if (!sampled) void writeDiagnostic('debug', 'ipc.started', `Invocando ${command}.`, { command, args });
  try {
    const result = await window.__TAURI__.core.invoke(command, args);
    if (!sampled) void writeDiagnostic('debug', 'ipc.completed', `${command} completado.`, {
      command,
      durationMs: Math.round(performance.now() - started)
    });
    return result;
  } catch (error) {
    addNotification(error, `${commandLabel(command)}: error`, command);
    void writeDiagnostic('error', 'ipc.failed', messageFrom(error, `${command} falló.`), {
      command,
      args,
      durationMs: Math.round(performance.now() - started)
    });
    throw error;
  }
}

function enqueueProcessingOperation(operation) {
  const queued = processingOperationQueue.catch(() => {}).then(operation);
  processingOperationQueue = queued;
  return queued;
}

async function writeDiagnostic(level, event, message, context = null) {
  if (!hasTauriRuntime()) return;
  try {
    await window.__TAURI__.core.invoke('write_frontend_log', {
      entry: {
        level,
        event,
        message,
        context: redactDiagnosticContext(context)
      }
    });
  } catch {
    // Diagnostics are auxiliary and must never interrupt a UI action.
  }
}

function setTheme(isDark) {
  document.documentElement.dataset.theme = isDark ? 'dark' : 'light';
  ui.theme.checked = isDark;
  writeStorage(themeStorageKey, isDark ? 'dark' : 'light');
  void writeDiagnostic('info', 'theme.changed', 'Tema de la interfaz actualizado.', { theme: isDark ? 'dark' : 'light' });
}

function setButtonVisual(button, label, iconHref) {
  const target = button.querySelector('[data-button-label]');
  if (target) target.textContent = label;
  else button.textContent = label;
  const icon = button.querySelector('[data-button-icon]');
  if (icon && iconHref) icon.setAttribute('href', iconHref);
  button.setAttribute('aria-label', label);
}

function setTextWithTitle(element, text, titleThreshold = 44) {
  element.textContent = text;
  if (String(text).length > titleThreshold) element.title = text;
  else element.removeAttribute('title');
}

function syncSelectTitle(select) {
  const text = select.selectedOptions[0]?.textContent?.trim() || '';
  if (text.length > 36) select.title = text;
  else select.removeAttribute('title');
}

function initializePanelState() {
  let stored = {};
  try {
    stored = JSON.parse(readStorage(panelStorageKey) || '{}');
  } catch {
    stored = {};
  }
  document.querySelectorAll('.settings-section[data-panel]').forEach((section) => {
    const key = section.dataset.panel;
    if (typeof stored[key] === 'boolean') section.open = stored[key];
    section.addEventListener('toggle', () => {
      let current = {};
      try {
        current = JSON.parse(readStorage(panelStorageKey) || '{}');
      } catch {
        current = {};
      }
      current[key] = section.open;
      writeStorage(panelStorageKey, JSON.stringify(current));
    });
  });
}

function inspectorWidthBounds() {
  return {
    minimum: 360,
    maximum: Math.max(360, Math.min(620, window.innerWidth - 560))
  };
}

function setInspectorWidth(width, persist = true) {
  const { minimum, maximum } = inspectorWidthBounds();
  const clamped = Math.round(Math.min(maximum, Math.max(minimum, Number(width) || 430)));
  document.documentElement.style.setProperty('--inspector-width', `${clamped}px`);
  ui.workspaceSplitter.setAttribute('aria-valuemin', String(minimum));
  ui.workspaceSplitter.setAttribute('aria-valuemax', String(maximum));
  ui.workspaceSplitter.setAttribute('aria-valuenow', String(clamped));
  if (persist) writeStorage(inspectorWidthStorageKey, String(clamped));
}

function initializeWorkspaceSplitter() {
  setInspectorWidth(Number(readStorage(inspectorWidthStorageKey) || 430), false);
  ui.workspaceSplitter.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    ui.workspaceSplitter.setPointerCapture(event.pointerId);
    document.body.classList.add('resizing');
  });
  ui.workspaceSplitter.addEventListener('pointermove', (event) => {
    if (!ui.workspaceSplitter.hasPointerCapture(event.pointerId)) return;
    const workspaceLeft = ui.workspace.getBoundingClientRect().left;
    setInspectorWidth(event.clientX - workspaceLeft, false);
  });
  const finishResize = (event) => {
    if (!ui.workspaceSplitter.hasPointerCapture(event.pointerId)) return;
    ui.workspaceSplitter.releasePointerCapture(event.pointerId);
    document.body.classList.remove('resizing');
    const current = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--inspector-width'));
    setInspectorWidth(current, true);
  };
  ui.workspaceSplitter.addEventListener('pointerup', finishResize);
  ui.workspaceSplitter.addEventListener('pointercancel', finishResize);
  ui.workspaceSplitter.addEventListener('lostpointercapture', () => document.body.classList.remove('resizing'));
  ui.workspaceSplitter.addEventListener('keydown', (event) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const current = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--inspector-width')) || 430;
    setInspectorWidth(current + (event.key === 'ArrowLeft' ? -16 : 16));
  });
  window.addEventListener('resize', () => {
    const current = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--inspector-width')) || 430;
    setInspectorWidth(current, false);
  });
}

function updateCameraState(text, online = false, isError = false) {
  ui.state.lastChild.textContent = ` ${text}`;
  if (text.length > 34) ui.state.title = text;
  else ui.state.removeAttribute('title');
  ui.state.classList.toggle('online', online);
  ui.state.classList.toggle('error', isError);
}

function reportError(error, fallback) {
  const message = messageFrom(error, fallback);
  addNotification(error, fallback || 'Error', 'interfaz');
  console.error(message, error);
  void writeDiagnostic('error', 'ui.error', message, {
    fallback,
    errorType: error?.constructor?.name,
    stack: error instanceof Error ? error.stack : undefined
  });
}

function loadNotifications() {
  try {
    const stored = JSON.parse(readStorage(notificationStorageKey) || '[]');
    notifications = normalizeNotifications(stored, maxNotifications);
  } catch {
    notifications = [];
  }
}

function notificationCode(message) {
  return message.match(/0x[0-9a-f]{8}/i)?.[0]?.toUpperCase()
    || message.match(/\b[A-Z][A-Z0-9_]{4,}\b/)?.[0]
    || null;
}

function saveNotifications() {
  writeStorage(notificationStorageKey, JSON.stringify(notifications.slice(0, maxNotifications)));
}

function addNotification(error, title = 'Error', source = 'aplicación') {
  const message = messageFrom(error, title).slice(0, 4096);
  const now = Date.now();
  const duplicate = notifications.find((item) => item.message === message && now - item.timestamp < 2000);
  if (duplicate) {
    if (title !== 'Error' && duplicate.title !== title) {
      duplicate.title = String(title).slice(0, 256);
      duplicate.source = String(source).slice(0, 128);
      saveNotifications();
      renderNotifications();
    }
    return duplicate;
  }
  const item = {
    id: `${now}-${Math.random().toString(36).slice(2, 8)}`,
    timestamp: now,
    title: String(title).slice(0, 256),
    message,
    source: String(source).slice(0, 128),
    code: notificationCode(message),
    read: false
  };
  notifications.unshift(item);
  notifications = notifications.slice(0, maxNotifications);
  saveNotifications();
  renderNotifications();
  showNotificationToast(item);
  return item;
}

function renderNotifications() {
  const unread = notifications.filter((item) => !item.read).length;
  ui.notificationCount.textContent = unread > 99 ? '99+' : String(unread);
  ui.notificationCount.hidden = unread === 0;
  ui.notificationToggle.setAttribute('aria-label', unread
    ? `Abrir notificaciones, ${unread} sin leer`
    : 'Abrir notificaciones');
  ui.notificationList.replaceChildren();
  if (!notifications.length) {
    const empty = document.createElement('p');
    empty.className = 'notification-empty';
    empty.textContent = 'No hay notificaciones.';
    ui.notificationList.append(empty);
    return;
  }
  notifications.forEach((item) => {
    const article = document.createElement('article');
    article.className = `notification-item${item.read ? '' : ' unread'}`;
    const heading = document.createElement('div');
    const title = document.createElement('strong');
    const displayTitle = friendlyNotificationTitle(item);
    title.textContent = displayTitle;
    if (displayTitle.length > 40) title.title = displayTitle;
    const time = document.createElement('time');
    time.dateTime = new Date(item.timestamp).toISOString();
    time.textContent = new Date(item.timestamp).toLocaleString([], { dateStyle: 'short', timeStyle: 'medium' });
    heading.append(title, time);
    const message = document.createElement('p');
    message.textContent = friendlyNotificationMessage(item.message);
    const meta = document.createElement('small');
    meta.textContent = [item.code, friendlyNotificationSource(item.source)].filter(Boolean).join(' · ');
    article.append(heading, message, meta);
    ui.notificationList.append(article);
  });
}

function showNotificationToast(item) {
  if (notificationToastTimer !== null) window.clearTimeout(notificationToastTimer);
  ui.notificationToast.querySelector('strong').textContent = friendlyNotificationTitle(item);
  const message = friendlyNotificationMessage(item.message);
  ui.notificationToast.querySelector('span').textContent = item.code ? `${item.code} · ${message}` : message;
  ui.notificationToast.hidden = false;
  requestAnimationFrame(() => ui.notificationToast.classList.add('visible'));
  notificationToastTimer = window.setTimeout(() => {
    ui.notificationToast.classList.remove('visible');
    window.setTimeout(() => { ui.notificationToast.hidden = true; }, 180);
  }, 5000);
}

function toggleNotificationPanel(force) {
  const show = typeof force === 'boolean' ? force : ui.notificationPanel.hidden;
  ui.notificationPanel.hidden = !show;
  ui.notificationPanel.setAttribute('aria-hidden', String(!show));
  ui.notificationToggle.setAttribute('aria-expanded', String(show));
  if (show) {
    notifications.forEach((item) => { item.read = true; });
    saveNotifications();
    renderNotifications();
  }
}

function paintSlider(slider) {
  const min = Number(slider.min);
  const max = Number(slider.max);
  const span = max - min;
  const percentage = span > 0 ? ((Number(slider.value) - min) / span) * 100 : 0;
  slider.style.setProperty('--progress', `${Math.min(100, Math.max(0, percentage))}%`);
}

function updateAvailability() {
  const hasCamera = Boolean(selectedCameraId);
  const transitioning = previewState === 'starting' || previewState === 'stopping'
    || virtualOutputState === 'starting' || virtualOutputState === 'stopping';
  const outputRunning = virtualOutputState === 'running';
  ui.saveProfile.disabled = !hasCamera;
  ui.deleteProfile.disabled = !hasCamera || !ui.profile.value;
  ui.restoreDefaults.disabled = !hasCamera || controls.length === 0;
  ui.driverProperties.disabled = !hasCamera || transitioning || outputRunning;
  ui.captureFormat.disabled = !hasCamera || transitioning || outputRunning || nativeFormats.length === 0;
  ui.previewToggle.disabled = !hasCamera || transitioning || outputRunning;
  ui.probeNativeBackend.disabled = !hasCamera || transitioning || nativeProbeRunning
    || previewState !== 'stopped' || virtualOutputState !== 'stopped' || nativeFormats.length === 0;
  ui.camera.disabled = transitioning;
  ui.refreshCameras.disabled = transitioning;
  ui.installVirtualCamera.disabled = transitioning || !virtualCameraSupported || virtualCameraInstalled;
  ui.removeVirtualCamera.disabled = transitioning || !virtualCameraSupported || !virtualCameraInstalled || outputRunning;
  ui.virtualOutputToggle.disabled = !hasCamera || transitioning || !virtualCameraInstalled;
  ui.outputQuality.disabled = transitioning || outputRunning;
  ui.controls.querySelectorAll('.control-row').forEach((row, index) => {
    const control = controls[index];
    const slider = row.querySelector('.slider');
    const numeric = row.querySelector('.value-box');
    const auto = row.querySelector('.auto-button');
    const reset = row.querySelector('.reset-button');
    if (slider) slider.disabled = transitioning || outputRunning || !control?.supportsManual || Boolean(control?.automatic);
    if (numeric) numeric.disabled = transitioning || outputRunning || !control?.supportsManual || Boolean(control?.automatic);
    if (auto) auto.disabled = transitioning || outputRunning || Boolean(control?.supportsAuto && !control?.supportsManual);
    if (reset) reset.disabled = transitioning || outputRunning;
  });
  ui.filterList.querySelectorAll('input, button').forEach((control) => {
    control.disabled = transitioning || control.dataset.graphDisabled === 'true';
  });
  ui.addFilter.disabled = transitioning;
  ui.filterPluginFile.disabled = transitioning || outputRunning;
}

const builtInFilters = [
  scalarFilter('brightness', 'Brillo', 'Suma luz digital a la imagen.', -0.5, 0.5, 0.005, 0, 'signedPercent'),
  scalarFilter('contrast', 'Contraste', 'Expande o comprime la diferencia tonal.', 0, 2, 0.005, 1, 'percent'),
  scalarFilter('saturation', 'Saturación', 'Controla la intensidad de los colores.', 0, 2, 0.005, 1, 'percent'),
  scalarFilter('gamma', 'Gamma', 'Modifica los tonos medios mediante una curva gamma.', 0.25, 2.5, 0.005, 1, 'decimal'),
  scalarFilter('temperature', 'Temperatura', 'Desplaza el balance entre azul y rojo.', -0.5, 0.5, 0.005, 0, 'signedPercent'),
  scalarFilter('tint', 'Matiz verde/magenta', 'Ajusta el eje verde y magenta.', -0.5, 0.5, 0.005, 0, 'signedPercent'),
  {
    type: 'flip',
    name: 'Volteo y espejo',
    description: 'Invierte la imagen horizontal o verticalmente.',
    create: () => ({ horizontal: true, vertical: false })
  },
  {
    type: 'lensCorrection',
    name: 'Corrección de lente',
    description: 'Corrige distorsión radial, tangencial y ojo de pez.',
    create: () => ({ k1: 0, k2: 0, k3: 0, p1: 0, p2: 0, scale: 0 }),
    parameters: [
      parameter('k1', 'Distorsión K1', -0.5, 0.5, 0.001, 'decimal'),
      parameter('k2', 'Distorsión K2', -0.25, 0.25, 0.001, 'decimal'),
      parameter('k3', 'Distorsión K3', -0.1, 0.1, 0.0005, 'decimal'),
      parameter('p1', 'Tangencial P1', -0.05, 0.05, 0.0005, 'decimal'),
      parameter('p2', 'Tangencial P2', -0.05, 0.05, 0.0005, 'decimal'),
      parameter('scale', 'Escala', -0.25, 0.5, 0.005, 'percent')
    ]
  },
  {
    type: 'lut3d',
    name: 'LUT 3D',
    description: 'Aplica un archivo .cube con intensidad regulable.',
    create: () => ({ strength: 1 }),
    parameters: [parameter('strength', 'Intensidad', 0, 1, 0.01, 'percent')]
  }
];

function parameter(key, label, minimum, maximum, step, format = 'decimal') {
  return { key, label, minimum, maximum, step, format };
}

function scalarFilter(type, name, description, minimum, maximum, step, defaultValue, format) {
  return {
    type,
    name,
    description,
    create: () => ({ amount: defaultValue }),
    parameters: [parameter('amount', name, minimum, maximum, step, format)]
  };
}

function pluginDescriptor(plugin) {
  return {
    type: 'plugin',
    pluginId: plugin.id,
    name: plugin.name,
    description: plugin.description || `${plugin.author || 'Plugin externo'} · ${plugin.version}`,
    plugin,
    create: () => ({
      pluginId: plugin.id,
      parameters: Object.fromEntries(plugin.parameters.map((item) => [item.id, item.defaultValue]))
    }),
    parameters: plugin.parameters.map((item) => parameter(
      item.id,
      item.label,
      item.minimum,
      item.maximum,
      item.step,
      'decimal'
    ))
  };
}

function filterDescriptor(node) {
  if (node.type === 'plugin') {
    const plugin = filterPlugins.find((item) => item.id === node.pluginId);
    return plugin ? pluginDescriptor(plugin) : {
      type: 'plugin',
      name: node.label || `Plugin no disponible: ${node.pluginId}`,
      description: 'Instala el manifiesto del plugin para editar sus parámetros.',
      parameters: []
    };
  }
  return builtInFilters.find((item) => item.type === node.type);
}

function nextFilterId(type) {
  filterSequence += 1;
  return `filter-${type}-${Date.now().toString(36)}-${filterSequence.toString(36)}`.slice(0, 64);
}

function createFilterNode(descriptor) {
  return {
    id: nextFilterId(descriptor.type === 'plugin' ? 'plugin' : descriptor.type),
    enabled: true,
    type: descriptor.type,
    ...descriptor.create()
  };
}

function nodeParameterValue(node, descriptor, key) {
  if (node.type === 'plugin') {
    const declared = descriptor.plugin?.parameters.find((item) => item.id === key);
    const raw = Number(node.parameters?.[key] ?? declared?.defaultValue ?? 0);
    return declared ? Math.min(declared.maximum, Math.max(declared.minimum, raw)) : raw;
  }
  return node[key];
}

function setNodeParameterValue(node, key, value) {
  if (node.type === 'plugin') {
    node.parameters ??= {};
    node.parameters[key] = value;
  } else {
    node[key] = value;
  }
}

function appendParameterControl(container, node, descriptor, definition) {
  const label = document.createElement('label');
  label.className = 'filter-parameter';
  const heading = document.createElement('span');
  heading.textContent = definition.label;
  const numeric = document.createElement('input');
  numeric.className = 'filter-value-input';
  numeric.type = 'number';
  numeric.min = String(definition.minimum);
  numeric.max = String(definition.maximum);
  numeric.step = String(definition.step);
  numeric.setAttribute('aria-label', `Valor exacto de ${descriptor.name}: ${definition.label}`);
  const rawValue = Number(nodeParameterValue(node, descriptor, definition.key));
  const value = Math.min(definition.maximum, Math.max(definition.minimum,
    Number.isFinite(rawValue) ? rawValue : 0));
  setNodeParameterValue(node, definition.key, value);
  numeric.value = String(value);
  heading.append(numeric);
  const slider = document.createElement('input');
  slider.className = 'slider';
  slider.type = 'range';
  slider.min = String(definition.minimum);
  slider.max = String(definition.maximum);
  slider.step = String(definition.step);
  slider.value = String(value);
  slider.setAttribute('aria-label', `${descriptor.name}: ${definition.label}`);
  paintSlider(slider);
  const applyValue = (next, immediate = false) => {
    if (!Number.isFinite(next)) return false;
    const clamped = Math.min(definition.maximum, Math.max(definition.minimum, next));
    setNodeParameterValue(node, definition.key, clamped);
    slider.value = String(clamped);
    numeric.value = String(clamped);
    paintSlider(slider);
    scheduleFilterGraph(immediate ? 0 : undefined);
    return true;
  };
  slider.addEventListener('input', () => {
    applyValue(Number(slider.value));
  });
  slider.addEventListener('change', () => scheduleFilterGraph(0));
  numeric.addEventListener('input', () => {
    if (numeric.value === '' || numeric.value === '-' || numeric.value === '.') return;
    const next = Number(numeric.value);
    if (Number.isFinite(next) && next >= definition.minimum && next <= definition.maximum) {
      setNodeParameterValue(node, definition.key, next);
      slider.value = String(next);
      paintSlider(slider);
      scheduleFilterGraph();
    }
  });
  numeric.addEventListener('change', () => {
    const next = Number(numeric.value);
    applyValue(Number.isFinite(next) ? next : nodeParameterValue(node, descriptor, definition.key), true);
  });
  label.append(heading, slider);
  container.append(label);
}

function appendFlipControls(container, node) {
  const row = document.createElement('div');
  row.className = 'filter-toggle-row';
  for (const [key, text] of [['horizontal', 'Espejo horizontal'], ['vertical', 'Voltear vertical']]) {
    const label = document.createElement('label');
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = Boolean(node[key]);
    input.addEventListener('change', () => {
      node[key] = input.checked;
      scheduleFilterGraph(0);
    });
    label.append(input, document.createTextNode(text));
    row.append(label);
  }
  container.append(row);
}

function appendLutControls(container, node) {
  const row = document.createElement('div');
  row.className = 'filter-lut-row';
  const status = document.createElement('span');
  status.textContent = node.name || 'Ningún archivo .cube cargado';
  const picker = document.createElement('label');
  picker.className = 'button button-quiet lut-picker';
  picker.append(document.createTextNode('Cargar .cube'));
  const input = document.createElement('input');
  input.type = 'file';
  input.accept = '.cube,text/plain';
  input.addEventListener('change', () => void loadNodeLut(node, input.files?.[0]));
  picker.append(input);
  const clear = document.createElement('button');
  clear.className = 'button button-quiet';
  clear.type = 'button';
  clear.textContent = 'Desvincular';
  clear.disabled = !node.assetId;
  clear.dataset.graphDisabled = String(!node.assetId);
  clear.addEventListener('click', () => {
    delete node.assetId;
    delete node.name;
    renderFilterGraph();
    scheduleFilterGraph(0);
  });
  row.append(status, picker, clear);
  container.prepend(row);
}

async function loadNodeLut(node, file) {
  if (!file) return;
  if (file.size > 8 * 1024 * 1024) {
    reportError('La LUT supera el límite de 8 MiB.', 'No se pudo cargar la LUT');
    return;
  }
  const assetId = `lut-${node.id}`.slice(0, 64);
  try {
    const cube = await file.text();
    await enqueueProcessingOperation(() => invoke('set_filter_lut_asset', { assetId, cube }));
    node.assetId = assetId;
    node.name = file.name.slice(0, 255);
    renderFilterGraph();
    await applyFilterGraph();
    void writeDiagnostic('info', 'processing.lut_loaded', 'LUT 3D vinculada a un nodo.', {
      nodeId: node.id,
      name: file.name,
      size: file.size
    });
  } catch (error) {
    reportError(error, 'No se pudo cargar la LUT');
  }
}

function moveFilterNode(id, targetId, after = false) {
  if (id === targetId) return;
  const sourceIndex = filterGraph.nodes.findIndex((node) => node.id === id);
  let targetIndex = filterGraph.nodes.findIndex((node) => node.id === targetId);
  if (sourceIndex < 0 || targetIndex < 0) return;
  const [node] = filterGraph.nodes.splice(sourceIndex, 1);
  if (sourceIndex < targetIndex) targetIndex -= 1;
  filterGraph.nodes.splice(targetIndex + Number(after), 0, node);
  renderFilterGraph();
  scheduleFilterGraph(0);
}

function moveFilterByOffset(id, offset) {
  const index = filterGraph.nodes.findIndex((node) => node.id === id);
  const target = filterGraph.nodes[index + offset];
  if (!target) return;
  moveFilterNode(id, target.id, offset > 0);
}

function removeFilterNode(id) {
  filterGraph.nodes = filterGraph.nodes.filter((node) => node.id !== id);
  renderFilterGraph();
  scheduleFilterGraph(0);
}

function renderFilterGraph() {
  ui.filterList.replaceChildren();
  filterGraph.nodes.forEach((node, index) => {
    const descriptor = filterDescriptor(node);
    if (!descriptor) return;
    const card = ui.filterTemplate.content.firstElementChild.cloneNode(true);
    card.dataset.nodeId = node.id;
    card.classList.toggle('disabled', !node.enabled);
    const [title, subtitle] = card.querySelectorAll('.filter-node-title *');
    title.textContent = node.label || descriptor.name;
    if ((node.label || descriptor.name).length > 24) title.title = node.label || descriptor.name;
    subtitle.textContent = descriptor.description;
    subtitle.title = descriptor.description;
    const enabled = card.querySelector('.node-enabled input');
    enabled.checked = node.enabled !== false;
    enabled.addEventListener('change', () => {
      node.enabled = enabled.checked;
      card.classList.toggle('disabled', !node.enabled);
      scheduleFilterGraph(0);
    });
    const [up, down] = card.querySelectorAll('.node-move');
    up.disabled = index === 0;
    down.disabled = index === filterGraph.nodes.length - 1;
    up.dataset.graphDisabled = String(index === 0);
    down.dataset.graphDisabled = String(index === filterGraph.nodes.length - 1);
    up.addEventListener('click', () => moveFilterByOffset(node.id, -1));
    down.addEventListener('click', () => moveFilterByOffset(node.id, 1));
    card.querySelector('.node-remove').addEventListener('click', () => removeFilterNode(node.id));
    const controlsContainer = card.querySelector('.filter-node-controls');
    descriptor.parameters?.forEach((definition) => appendParameterControl(
      controlsContainer,
      node,
      descriptor,
      definition
    ));
    if (node.type === 'flip') appendFlipControls(controlsContainer, node);
    if (node.type === 'lut3d') appendLutControls(controlsContainer, node);
    ui.filterList.append(card);
  });
  updateAvailability();
}

async function applyFilterGraph() {
  if (!hasTauriRuntime()) return;
  // Send a validated snapshot without replacing the live objects captured by
  // the slider handlers. Replacing them here left the DOM bound to stale nodes,
  // so only the first pointer event could ever reach the native pipeline.
  const graph = normalizeFilterGraph(filterGraph);
  try {
    await enqueueProcessingOperation(() => invoke('set_filter_graph', { graph }));
  } catch (error) {
    reportError(error, 'No se pudo aplicar el grafo de filtros');
  }
}

function scheduleFilterGraph(delay = 80) {
  if (processingTimer !== null) window.clearTimeout(processingTimer);
  processingTimer = window.setTimeout(() => {
    processingTimer = null;
    void applyFilterGraph();
  }, delay);
}

function renderFilterCatalog() {
  ui.filterCatalogNative.replaceChildren();
  ui.filterCatalogPlugins.replaceChildren();
  const appendDescriptor = (descriptor, container) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = `catalog-item${descriptor.plugin ? ' plugin' : ''}`;
    const title = document.createElement('strong');
    title.textContent = descriptor.name;
    const description = document.createElement('span');
    description.textContent = descriptor.description;
    button.append(title, description);
    button.addEventListener('click', () => {
      filterGraph.nodes.push(createFilterNode(descriptor));
      ui.filterCatalog.close();
      renderFilterGraph();
      scheduleFilterGraph(0);
    });
    container.append(button);
  };
  builtInFilters.forEach((descriptor) => appendDescriptor(descriptor, ui.filterCatalogNative));
  filterPlugins.map(pluginDescriptor).forEach((descriptor) => appendDescriptor(descriptor, ui.filterCatalogPlugins));
  ui.filterPluginsEmpty.hidden = filterPlugins.length > 0;
}

async function initializeFilterSystem() {
  if (hasTauriRuntime()) {
    try {
      await refreshFilterPlugins();
      filterGraph = normalizeFilterGraph(await invoke('get_filter_graph'));
    } catch (error) {
      reportError(error, 'No se pudo inicializar el sistema de filtros');
    }
  }
  renderFilterCatalog();
  renderFilterGraph();
}

async function refreshFilterPlugins() {
  if (!hasTauriRuntime()) return;
  const catalog = await invoke('list_filter_plugins');
  filterPlugins = Array.isArray(catalog?.plugins) ? catalog.plugins : [];
  if (catalog?.warnings?.length) {
    void writeDiagnostic('warn', 'plugins.manifest_warnings', 'Algunos plugins fueron rechazados.', {
      warnings: catalog.warnings
    });
  }
  renderFilterCatalog();
}

async function installFilterPlugin(file) {
  if (!file) return;
  if (file.size > 256 * 1024) {
    reportError('El manifiesto supera el límite de 256 KiB.', 'No se pudo instalar el plugin');
    return;
  }
  const resumePreview = previewState === 'running';
  try {
    if (resumePreview) await stopPreview();
    const manifestJson = await file.text();
    const installed = await invoke('install_filter_plugin', { fileName: file.name, manifestJson });
    await refreshFilterPlugins();
    void writeDiagnostic('info', 'plugins.installed', 'Plugin instalado y persistido.', {
      id: installed.id,
      name: installed.name,
      fileName: file.name
    });
  } catch (error) {
    reportError(error, 'No se pudo instalar el plugin');
  } finally {
    ui.filterPluginFile.value = '';
    if (resumePreview && selectedCameraId && previewState === 'stopped') await startPreview();
  }
}

function renderControls() {
  const template = document.querySelector('#control-template');
  ui.controls.replaceChildren();
  if (!controls.length) {
    const empty = document.createElement('p');
    empty.className = 'empty-controls';
    empty.textContent = 'Selecciona una cámara para cargar sus ajustes.';
    ui.controls.append(empty);
    updateAvailability();
    return;
  }

  controls.forEach((control) => {
    const row = template.content.firstElementChild.cloneNode(true);
    const [name, range] = row.querySelectorAll('.control-name *');
    const slider = row.querySelector('.slider');
    const value = row.querySelector('.value-box');
    const auto = row.querySelector('.auto-button');
    const reset = row.querySelector('.reset-button');
    name.textContent = control.name;
    if (control.name.length > 22) name.title = control.name;
    const automaticOnly = control.supportsAuto && !control.supportsManual;
    row.classList.toggle('automatic-only', automaticOnly);
    range.textContent = automaticOnly ? 'Solo automático' : `${control.minimum} — ${control.maximum}`;
    slider.min = control.minimum;
    slider.max = control.maximum;
    slider.step = Math.max(1, Math.abs(Number(control.step) || 1));
    slider.value = control.value;
    slider.disabled = !control.supportsManual || control.automatic;
    slider.hidden = automaticOnly;
    slider.setAttribute('aria-label', `${control.name}, valor manual`);
    paintSlider(slider);
    value.min = control.minimum;
    value.max = control.maximum;
    value.step = slider.step;
    value.value = control.value;
    value.disabled = automaticOnly || control.automatic;
    value.hidden = automaticOnly;
    value.setAttribute('aria-label', `${control.name}, valor exacto`);
    auto.hidden = !control.supportsAuto;
    auto.disabled = automaticOnly;
    auto.textContent = automaticOnly ? 'Automático' : 'Auto';
    auto.classList.toggle('selected', Boolean(control.automatic));
    auto.setAttribute('aria-pressed', String(Boolean(control.automatic)));
    auto.setAttribute('aria-label', `${control.name}, modo automático`);
    reset.setAttribute('aria-label', `Restablecer ${control.name} al valor predeterminado`);

    slider.addEventListener('input', () => {
      control.value = Number(slider.value);
      control.automatic = false;
      paintSlider(slider);
      value.value = String(control.value);
      auto.classList.remove('selected');
      auto.setAttribute('aria-pressed', 'false');
      scheduleControl(control, 140);
    });
    slider.addEventListener('change', () => scheduleControl(control, 0));
    value.addEventListener('input', () => {
      if (value.value === '' || value.value === '-' || value.value === '.') return;
      const next = Number(value.value);
      if (!Number.isFinite(next) || next < control.minimum || next > control.maximum) return;
      control.value = clampControlValue(control, next);
      control.automatic = false;
      slider.value = String(control.value);
      paintSlider(slider);
      auto.classList.remove('selected');
      auto.setAttribute('aria-pressed', 'false');
      scheduleControl(control, 140);
    });
    value.addEventListener('change', () => {
      const next = Number(value.value);
      control.value = clampControlValue(control, Number.isFinite(next) ? next : control.value);
      value.value = String(control.value);
      slider.value = String(control.value);
      paintSlider(slider);
      scheduleControl(control, 0);
    });
    auto.addEventListener('click', async () => {
      if (!control.supportsManual) return;
      cancelScheduledControl(control.id);
      const cameraId = selectedCameraId;
      const revision = cameraRevision;
      control.automatic = !control.automatic;
      auto.classList.toggle('selected', control.automatic);
      auto.setAttribute('aria-pressed', String(control.automatic));
      updateAvailability();
      try {
        await enqueueControlOperation(() => applyControl(control, cameraId, revision));
      } catch (error) {
        reportError(error, `No se pudo cambiar ${control.name}`);
        await loadControls();
      }
    });
    reset.addEventListener('click', async () => {
      cancelScheduledControl(control.id);
      const cameraId = selectedCameraId;
      const revision = cameraRevision;
      control.value = control.defaultValue;
      control.automatic = Boolean(control.defaultAutomatic && control.supportsAuto);
      try {
        await enqueueControlOperation(() => applyControl(control, cameraId, revision));
        await loadControls();
      } catch (error) {
        reportError(error, `No se pudo restablecer ${control.name}`);
        await loadControls();
      }
    });
    ui.controls.append(row);
  });
  updateAvailability();
}

async function withPreviewPaused(operation, reason) {
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  const resumePreview = previewState === 'running';
  if (resumePreview) {
    void writeDiagnostic('info', 'preview.paused_for_control', 'Vista previa pausada para cambiar un control.', { cameraId, reason });
    await stopPreview();
  }
  try {
    return await operation();
  } finally {
    if (resumePreview && isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) {
      await startPreview();
      void writeDiagnostic('info', 'preview.resumed_after_control', 'Vista previa reanudada después del cambio.', { cameraId, reason });
    }
  }
}

async function applyControl(control, cameraId, revision, pausePreview = true) {
  if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return false;
  const apply = () => invoke('set_control', {
      cameraId,
      kind: control.kind,
      property: control.property,
      value: control.value,
      automatic: control.automatic
    });
  if (pausePreview) await withPreviewPaused(apply, control.id);
  else await apply();
  if (isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) {
    updateCameraState('Cambios aplicados', true);
  }
  return true;
}

function scheduleControl(control, delay) {
  cancelScheduledControl(control.id);
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  const timer = window.setTimeout(async () => {
    scheduledControls.delete(control.id);
    try {
      await enqueueControlOperation(() => applyControl(control, cameraId, revision));
    } catch (error) {
      reportError(error, `No se pudo aplicar ${control.name}`);
      if (isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) await loadControls();
    }
  }, delay);
  scheduledControls.set(control.id, timer);
}

function cancelScheduledControl(controlId) {
  const scheduled = scheduledControls.get(controlId);
  if (scheduled !== undefined) window.clearTimeout(scheduled);
  scheduledControls.delete(controlId);
}

function cancelAllScheduledControls() {
  for (const timer of scheduledControls.values()) window.clearTimeout(timer);
  scheduledControls.clear();
}

function enqueueControlOperation(operation) {
  const queued = controlOperationQueue
    .catch(() => undefined)
    .then(operation);
  controlOperationQueue = queued;
  return queued;
}

async function loadControls() {
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  if (!cameraId) {
    controls = [];
    renderControls();
    updateCameraState('Sin cámara seleccionada');
    return;
  }

  updateCameraState('Leyendo ajustes del controlador…');
  try {
    const readControls = () => invoke('get_controls', { cameraId });
    const response = previewState === 'running'
      ? await withPreviewPaused(readControls, 'read_controls')
      : await readControls();
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
    if (!Array.isArray(response)) throw new Error('El motor devolvió controles inválidos.');
    controls = response;
    renderControls();
    updateCameraState(`${controls.length} ajustes disponibles`, true);
  } catch (error) {
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
    controls = [];
    renderControls();
    reportError(error, 'No se pudieron leer los ajustes de la cámara');
  }
}

async function loadNativeFormats(cameraId = selectedCameraId, revision = cameraRevision) {
  nativeFormats = [];
  renderCaptureFormats(false);
  ui.nativeFormatSummary.textContent = cameraId
    ? 'Consultando modos nativos…'
    : 'Sin cámara seleccionada';
  if (!cameraId || !hasTauriRuntime()) return;
  try {
    const response = await invoke('list_native_formats', { cameraId });
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
    if (!Array.isArray(response)) throw new Error('camera-host devolvió formatos inválidos.');
    nativeFormats = sortVideoFormats(response);
    renderCaptureFormats();
    ui.nativeFormatSummary.textContent = summarizeVideoFormats(nativeFormats);
  } catch (error) {
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
    nativeFormats = [];
    renderCaptureFormats(false);
    ui.nativeFormatSummary.textContent = 'Modos nativos no disponibles';
    void writeDiagnostic('warn', 'camera.native_formats_failed', messageFrom(error), { cameraId });
  } finally {
    updateAvailability();
  }
}

function readCaptureFormatPreferences() {
  try {
    const value = JSON.parse(readStorage(captureFormatStorageKey) || '{}');
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
    const preferences = {};
    for (const [cameraId, formatKey] of Object.entries(value).slice(0, 64)) {
      if (!cameraId || cameraId.length > 2048 || typeof formatKey !== 'string') continue;
      Object.defineProperty(preferences, cameraId, {
        configurable: true,
        enumerable: true,
        value: formatKey.slice(0, 512),
        writable: true
      });
    }
    return preferences;
  } catch (error) {
    void writeDiagnostic('warn', 'capture_format.preferences_invalid', messageFrom(error));
    return {};
  }
}

function saveCaptureFormatPreference(cameraId, key) {
  if (!cameraId) return;
  const preferences = readCaptureFormatPreferences();
  if (key) Object.defineProperty(preferences, cameraId, {
    configurable: true,
    enumerable: true,
    value: key,
    writable: true
  });
  else delete preferences[cameraId];
  writeStorage(captureFormatStorageKey, JSON.stringify(preferences));
}

function renderCaptureFormats(validatePreference = true) {
  const preferences = readCaptureFormatPreferences();
  const preferredKey = selectedCameraId && Object.hasOwn(preferences, selectedCameraId)
    ? String(preferences[selectedCameraId] || '')
    : '';
  ui.captureFormat.replaceChildren(new Option('Automático (recomendado)', ''));
  nativeFormats.forEach((format) => {
    ui.captureFormat.add(new Option(formatVideoMode(format), videoFormatKey(format)));
  });
  const available = nativeFormats.some((format) => videoFormatKey(format) === preferredKey);
  ui.captureFormat.value = available ? preferredKey : '';
  syncSelectTitle(ui.captureFormat);
  if (validatePreference && preferredKey && !available) saveCaptureFormatPreference(selectedCameraId, '');
}

function selectedCaptureFormat() {
  const key = ui.captureFormat.value;
  return key ? nativeFormats.find((format) => videoFormatKey(format) === key) ?? null : null;
}

const virtualOutputSizes = [[640, 360], [640, 480], [1280, 720], [1920, 1080], [2560, 1440], [3840, 2160]];

function nearestVirtualOutputSize(format) {
  if (!format) return null;
  const exact = virtualOutputSizes.find(([width, height]) => width === format.width && height === format.height);
  if (exact) return exact;
  const aspect = format.width / Math.max(1, format.height);
  return [...virtualOutputSizes].sort((left, right) => {
    const leftAspect = Math.abs(left[0] / left[1] - aspect);
    const rightAspect = Math.abs(right[0] / right[1] - aspect);
    if (leftAspect !== rightAspect) return leftAspect - rightAspect;
    return Math.abs(left[0] * left[1] - format.width * format.height)
      - Math.abs(right[0] * right[1] - format.width * format.height);
  })[0];
}

function automaticVirtualInputFormat() {
  return preferredProbeFormat(nativeFormats);
}

function virtualOutputSelection() {
  const inputFormat = selectedCaptureFormat() ?? automaticVirtualInputFormat();
  const size = nearestVirtualOutputSize(inputFormat);
  return inputFormat && size ? { inputFormat, width: size[0], height: size[1] } : null;
}

async function changeCaptureFormat() {
  if (!selectedCameraId) return;
  const format = selectedCaptureFormat();
  saveCaptureFormatPreference(selectedCameraId, format ? videoFormatKey(format) : '');
  void writeDiagnostic('info', 'capture_format.changed', 'Formato nativo de captura modificado.', {
    mode: format ? formatVideoMode(format) : 'automatic'
  });
  if (previewState === 'running') {
    await stopPreview();
    await startPreview();
  }
}

function preferredProbeFormat(formats) {
  const priority = ['NV12', 'YUY2', 'MJPEG', 'H264', 'BGRA'];
  return [...formats].sort((left, right) => {
    const leftFps = left.fpsNumerator / Math.max(1, left.fpsDenominator);
    const rightFps = right.fpsNumerator / Math.max(1, right.fpsDenominator);
    const left1080 = left.width === 1920 && left.height === 1080 && leftFps <= 30 ? 0 : 1;
    const right1080 = right.width === 1920 && right.height === 1080 && rightFps <= 30 ? 0 : 1;
    if (left1080 !== right1080) return left1080 - right1080;
    const formatOrder = priority.indexOf(left.pixelFormat) - priority.indexOf(right.pixelFormat);
    if (formatOrder !== 0) return formatOrder;
    return right.width * right.height - left.width * left.height;
  })[0];
}

async function probeNativeBackend() {
  if (!selectedCameraId || !nativeFormats.length || previewState !== 'stopped' || virtualOutputState !== 'stopped') return;
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  const format = selectedCaptureFormat() ?? preferredProbeFormat(nativeFormats);
  nativeProbeRunning = true;
  updateAvailability();
  ui.nativeFormatSummary.textContent = `Probando WinRT ${format.width}×${format.height}…`;
  try {
    const result = await invoke('probe_media_frame_reader', { cameraId, format, frames: 30 });
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
    const fps = measuredProbeFps(result);
    const measuredFps = fps === null ? '—' : fps.toFixed(1);
    ui.nativeFormatSummary.textContent = `WinRT OK · primer frame ${result.firstFrameMillis} ms · ${measuredFps} FPS`;
  } catch (error) {
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
    ui.nativeFormatSummary.textContent = 'WinRT falló · consulta los registros';
    reportError(error, 'La prueba MediaFrameReader falló');
  } finally {
    nativeProbeRunning = false;
    updateAvailability();
  }
}

async function selectCamera(cameraId) {
  void writeDiagnostic('info', 'camera.selection_changed', 'El usuario cambió la cámara seleccionada.', {
    cameraId,
    cameraName: cameras.find((camera) => camera.id === cameraId)?.name
  });
  cancelAllScheduledControls();
  cameraRevision += 1;
  const revision = cameraRevision;
  selectedCameraId = cameraId;
  ui.camera.value = cameraId;
  controls = [];
  nativeFormats = [];
  renderCaptureFormats(false);
  renderControls();
  renderProfiles();
  updateCameraState('Cambiando cámara…');
  await stopVirtualOutput();
  await stopPreview();
  if (revision !== cameraRevision) return;
  await loadControls();
  await loadNativeFormats(cameraId, revision);
}

async function refreshCameras() {
  const previousCameraId = selectedCameraId;
  cancelAllScheduledControls();
  cameraRevision += 1;
  const revision = cameraRevision;
  await stopVirtualOutput();
  await stopPreview();
  updateCameraState('Buscando cámaras…');
  ui.refreshCameras.disabled = true;
  ui.camera.disabled = true;
  try {
    if (!hasTauriRuntime()) {
      cameras = [];
      selectedCameraId = '';
      nativeFormats = [];
      renderCaptureFormats(false);
      ui.camera.replaceChildren(new Option('Ejecuta la aplicación Tauri para conectar la cámara', ''));
      controls = [];
      renderControls();
      renderProfiles();
      updateCameraState('Modo de diseño');
      return;
    }

    const response = await invoke('list_cameras');
    if (revision !== cameraRevision) return;
    if (!Array.isArray(response)) throw new Error('El motor devolvió una lista de cámaras inválida.');
    cameras = response;
    try {
      const nativeResponse = await invoke('list_native_cameras');
      nativeCameras = Array.isArray(nativeResponse) ? nativeResponse : [];
      const directShowIds = new Set(cameras.map((camera) => camera.id));
      const unmatched = nativeCameras.filter((camera) => !directShowIds.has(camera.id));
      void writeDiagnostic('info', 'camera.native_inventory', 'Inventario Media Foundation comparado.', {
        directShowCount: cameras.length,
        mediaFoundationCount: nativeCameras.length,
        unmatchedCount: unmatched.length
      });
    } catch (error) {
      nativeCameras = [];
      void writeDiagnostic('warn', 'camera.native_inventory_failed', messageFrom(error));
    }
    ui.camera.replaceChildren();
    if (cameras.length === 0) {
      ui.camera.add(new Option('No se detectaron cámaras', ''));
      selectedCameraId = '';
      nativeFormats = [];
      renderCaptureFormats(false);
      controls = [];
      renderControls();
      renderProfiles();
      updateCameraState('Sin cámara detectada');
      return;
    }

    cameras.forEach((camera) => ui.camera.add(new Option(camera.name, camera.id)));
    selectedCameraId = cameras.some((camera) => camera.id === previousCameraId)
      ? previousCameraId
      : cameras[0].id;
    ui.camera.value = selectedCameraId;
    renderProfiles();
    await loadControls();
    await loadNativeFormats(selectedCameraId, revision);
  } catch (error) {
    cameras = [];
    nativeCameras = [];
    nativeFormats = [];
    selectedCameraId = '';
    renderCaptureFormats(false);
    controls = [];
    ui.camera.replaceChildren(new Option('No se pudieron cargar las cámaras', ''));
    ui.nativeFormatSummary.textContent = 'Modos nativos no disponibles';
    renderControls();
    renderProfiles();
    reportError(error, 'No se pudieron buscar cámaras');
  } finally {
    syncSelectTitle(ui.camera);
    updateAvailability();
  }
}

function readProfileStore() {
  try {
    const serialized = readStorage(profileStorageKey)
      ?? legacyProfileStorageKeys.map(readStorage).find(Boolean);
    const store = normalizeProfileStore(JSON.parse(serialized || 'null'));
    if (serialized && !readStorage(profileStorageKey)) writeProfileStore(store);
    return store;
  } catch (error) {
    console.warn('No se pudieron leer los perfiles guardados.', error);
    void writeDiagnostic('warn', 'profiles.read_failed', messageFrom(error), {
      errorType: error?.constructor?.name
    });
  }
  return emptyProfileStore();
}

function writeProfileStore(store) {
  return writeStorage(profileStorageKey, JSON.stringify(store));
}

function renderProfiles() {
  const profiles = profilesForCamera(readProfileStore(), selectedCameraId);
  const current = ui.profile.value;
  ui.profile.replaceChildren(new Option('Sin perfil seleccionado', ''));
  Object.keys(profiles).sort((a, b) => a.localeCompare(b)).forEach((name) => {
    ui.profile.add(new Option(name, name));
  });
  ui.profile.value = Object.hasOwn(profiles, current) ? current : '';
  syncSelectTitle(ui.profile);
  updateAvailability();
}

function profileNameExists(name) {
  if (!selectedCameraId || !name) return false;
  return Object.hasOwn(profilesForCamera(readProfileStore(), selectedCameraId), name);
}

function updateProfileDialog() {
  const name = ui.profileName.value.trim().slice(0, 80);
  const replacing = profileNameExists(name);
  ui.profileWarning.hidden = !replacing;
  ui.profileSubmit.querySelector('span').textContent = replacing ? 'Reemplazar perfil' : 'Guardar perfil';
}

function saveProfile() {
  if (!selectedCameraId) return;
  profileDialogReturnFocus = document.activeElement;
  ui.profileName.value = ui.profile.value || '';
  updateProfileDialog();
  ui.profileDialog.showModal();
  requestAnimationFrame(() => {
    ui.profileName.focus();
    ui.profileName.select();
  });
}

function commitProfile(name) {
  name = name?.trim().slice(0, 80);
  if (!name) return false;
  const store = readProfileStore();
  const profiles = profilesForCamera(store, selectedCameraId, true);
  Object.defineProperty(profiles, name, {
    configurable: true,
    enumerable: true,
    writable: true,
    value: {
      controls: controls.map(({ id, value, automatic }) => ({ id, value, automatic })),
      filterGraph: normalizeFilterGraph(filterGraph)
    }
  });
  if (!writeProfileStore(store)) {
    reportError('El almacenamiento local no está disponible.', 'No se pudo guardar el perfil');
    return false;
  }
  void writeDiagnostic('info', 'profile.saved', 'Perfil de cámara guardado.', {
    cameraId: selectedCameraId,
    profileName: name,
    controlCount: controls.length
  });
  renderProfiles();
  ui.profile.value = name;
  syncSelectTitle(ui.profile);
  updateAvailability();
  return true;
}

function requestConfirmation({ title, message, confirmLabel = 'Confirmar' }) {
  const returnFocus = document.activeElement;
  ui.confirmTitle.textContent = title;
  ui.confirmMessage.textContent = message;
  ui.confirmSubmit.textContent = confirmLabel;
  ui.confirmDialog.returnValue = '';
  ui.confirmDialog.showModal();
  requestAnimationFrame(() => ui.confirmCancel.focus());
  return new Promise((resolve) => {
    ui.confirmDialog.addEventListener('close', () => {
      if (returnFocus instanceof HTMLElement && returnFocus.isConnected) returnFocus.focus();
      resolve(ui.confirmDialog.returnValue === 'confirm');
    }, { once: true });
  });
}

async function applyProfile() {
  const profileName = ui.profile.value;
  const profiles = profilesForCamera(readProfileStore(), selectedCameraId);
  const profile = Object.hasOwn(profiles, profileName) ? profiles[profileName] : undefined;
  const savedControls = Array.isArray(profile) ? profile : profile?.controls;
  if (!Array.isArray(savedControls)) {
    updateAvailability();
    return;
  }

  cancelAllScheduledControls();
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  const failures = [];
  void writeDiagnostic('info', 'profile.apply_started', 'Aplicación de perfil iniciada.', {
    cameraId: selectedCameraId,
    profileName,
    controlCount: savedControls.length
  });
  updateCameraState(`Aplicando perfil “${profileName}”…`);
  const resumePreview = previewState === 'running';
  if (resumePreview) await stopPreview();
  try {
    for (const saved of savedControls) {
      if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) return;
      const control = controls.find((item) => item.id === saved.id);
      if (!control) continue;
      control.value = clampControlValue(control, saved.value);
      control.automatic = Boolean(saved.automatic && control.supportsAuto);
      try {
        await applyControl(control, cameraId, revision, false);
      } catch (error) {
        failures.push(`${control.name}: ${messageFrom(error)}`);
      }
    }
    filterGraph = normalizeFilterGraph(profile?.filterGraph);
    renderFilterGraph();
    await applyFilterGraph();
    await loadControls();
  } finally {
    if (resumePreview && isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) await startPreview();
  }
  if (failures.length) reportError(failures.join(' · '), 'Algunos controles no se pudieron aplicar');
  else void writeDiagnostic('info', 'profile.apply_completed', 'Perfil aplicado correctamente.', {
    cameraId,
    profileName
  });
}

async function deleteProfile() {
  const name = ui.profile.value;
  if (!selectedCameraId || !name) return;
  if (!await requestConfirmation({
    title: 'Eliminar perfil',
    message: `Se eliminará el perfil “${name}” de esta cámara. Esta acción no modifica la imagen actual.`,
    confirmLabel: 'Eliminar perfil'
  })) return;
  const store = readProfileStore();
  const profiles = profilesForCamera(store, selectedCameraId);
  delete profiles[name];
  if (!writeProfileStore(store)) {
    reportError('El almacenamiento local no está disponible.', 'No se pudo eliminar el perfil');
    return;
  }
  void writeDiagnostic('info', 'profile.deleted', 'Perfil de cámara eliminado.', {
    cameraId: selectedCameraId,
    profileName: name
  });
  renderProfiles();
}

async function restoreDefaults() {
  if (!controls.length) return;
  cancelAllScheduledControls();
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  const failures = [];
  updateCameraState('Restableciendo valores predeterminados…');
  const resumePreview = previewState === 'running';
  if (resumePreview) await stopPreview();
  try {
    for (const control of controls) {
      control.value = control.defaultValue;
      control.automatic = Boolean(control.defaultAutomatic && control.supportsAuto);
      try {
        await applyControl(control, cameraId, revision, false);
      } catch (error) {
        failures.push(`${control.name}: ${messageFrom(error)}`);
      }
    }
    await loadControls();
  } finally {
    if (resumePreview && isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) await startPreview();
  }
  if (failures.length) reportError(failures.join(' · '), 'Algunos controles no se pudieron restablecer');
}

async function openDriverProperties() {
  if (!selectedCameraId || virtualOutputState === 'running') return;
  const cameraId = selectedCameraId;
  updateCameraState('Abriendo panel original del fabricante…');
  try {
    await withPreviewPaused(
      () => invoke('open_driver_property_page', { cameraId }),
      'driver_property_page'
    );
    await loadControls();
  } catch (error) {
    reportError(error, 'No se pudo abrir el panel del fabricante');
  }
}

function setPreviewState(state) {
  previewState = state;
  const running = state === 'running';
  ui.previewStage.classList.toggle('active', running);
  ui.previewEmpty.hidden = running;
  setButtonVisual(
    ui.previewToggle,
    running ? 'Detener vista previa' : state === 'starting' ? 'Iniciando…' : state === 'stopping' ? 'Deteniendo…' : 'Iniciar vista previa',
    running || state === 'stopping' ? '#tabler-player-stop' : '#tabler-player-play'
  );
  updateAvailability();
}

function clearPreviewImage() {
  ui.previewImage.removeAttribute('src');
  ui.previewImage.hidden = true;
  if (previewObjectUrl) URL.revokeObjectURL(previewObjectUrl);
  previewObjectUrl = null;
  activePreviewFormat = null;
}

async function startPreview() {
  if (!selectedCameraId || previewState !== 'stopped') return;
  const cameraId = selectedCameraId;
  const revision = cameraRevision;
  setPreviewState('starting');
    setTextWithTitle(ui.previewStatus, 'Conectando cámara…');
  try {
    const requestedFormat = selectedCaptureFormat();
    const result = await invoke('start_preview', { cameraId, requestedFormat });
    if (!isCurrentCameraRequest(cameraId, revision, selectedCameraId, cameraRevision)) {
      await invoke('stop_preview');
      setPreviewState('stopped');
      return;
    }
    activePreviewFormat = result?.format ?? requestedFormat;
    setTextWithTitle(ui.previewMode, activePreviewFormat
      ? `Entrada ${formatVideoMode(activePreviewFormat)} · monitor ${result.previewWidth}×${result.previewHeight}`
      : 'Vista previa optimizada');
    setPreviewState('running');
    previewTimer = window.setTimeout(refreshPreviewFrame, 80);
  } catch (error) {
    setPreviewState('stopped');
    setTextWithTitle(ui.previewStatus, 'No se pudo iniciar · revisa notificaciones');
    setTextWithTitle(ui.previewMode, 'Vista previa optimizada');
    reportError(error, 'No se pudo iniciar la vista previa');
  }
}

async function refreshPreviewFrame() {
  if (previewState !== 'running') return;
  try {
    const frame = await invoke('get_preview_frame');
    const bytes = frame instanceof ArrayBuffer
      ? new Uint8Array(frame)
      : frame instanceof Uint8Array
        ? frame
        : Array.isArray(frame)
          ? new Uint8Array(frame)
          : new Uint8Array();
    if (bytes.byteLength > 0) {
      const nextUrl = URL.createObjectURL(new Blob([bytes], { type: 'image/jpeg' }));
      ui.previewImage.src = nextUrl;
      ui.previewImage.hidden = false;
      if (previewObjectUrl) URL.revokeObjectURL(previewObjectUrl);
      previewObjectUrl = nextUrl;
      setTextWithTitle(ui.previewStatus, 'En directo');
    }
  } catch (error) {
    setTextWithTitle(ui.previewStatus, 'La captura se detuvo · revisa notificaciones');
    reportError(error, 'La captura se detuvo');
    await stopPreview();
    return;
  }
  if (previewState === 'running') previewTimer = window.setTimeout(refreshPreviewFrame, 25);
}

async function stopPreview() {
  if (previewState === 'stopped') return;
  setPreviewState('stopping');
  if (previewTimer !== null) window.clearTimeout(previewTimer);
  previewTimer = null;
  try {
    if (hasTauriRuntime()) await invoke('stop_preview');
  } catch (error) {
    reportError(error, 'No se pudo detener completamente la vista previa');
  } finally {
    clearPreviewImage();
    ui.previewStage.classList.remove('active');
    ui.previewEmpty.hidden = false;
    setTextWithTitle(ui.previewStatus, 'Sin transmisión');
    setTextWithTitle(ui.previewMode, 'Vista previa optimizada');
    setPreviewState('stopped');
  }
}

async function togglePreview() {
  if (previewState === 'running') await stopPreview();
  else if (previewState === 'stopped') await startPreview();
}

function updateVirtualCameraState(text, online = false, isError = false) {
  ui.virtualCameraState.lastChild.textContent = ` ${text}`;
  if (text.length > 34) ui.virtualCameraState.title = text;
  else ui.virtualCameraState.removeAttribute('title');
  ui.virtualCameraState.classList.toggle('online', online);
  ui.virtualCameraState.classList.toggle('error', isError);
}

function setVirtualOutputState(state) {
  virtualOutputState = state;
  const running = state === 'running';
  setButtonVisual(
    ui.virtualOutputToggle,
    running ? 'Detener salida' : state === 'starting' ? 'Activando…' : state === 'stopping' ? 'Deteniendo…' : 'Activar salida',
    running || state === 'stopping' ? '#tabler-player-stop' : '#tabler-broadcast'
  );
  updateAvailability();
}

async function refreshVirtualCameraStatus() {
  if (!hasTauriRuntime()) {
    virtualCameraSupported = false;
    virtualCameraInstalled = false;
    updateVirtualCameraState('Disponible dentro de la aplicación Tauri');
    updateAvailability();
    return;
  }
  try {
    const status = await invoke('get_virtual_camera_status');
    virtualCameraSupported = Boolean(status?.supported);
    virtualCameraInstalled = Boolean(status?.installed);
    if (!virtualCameraSupported) {
      updateVirtualCameraState('Componente nativo no disponible', false, true);
      if (status?.detail) addNotification(status.detail, 'Cámara virtual no disponible', 'cámara virtual');
    } else if (virtualCameraInstalled) {
      updateVirtualCameraState(status?.running ? 'Salida activa' : 'Instalada y lista', true);
      if (status?.running) setVirtualOutputState('running');
    } else {
      updateVirtualCameraState('Lista para instalar');
    }
  } catch (error) {
    virtualCameraSupported = false;
    virtualCameraInstalled = false;
    updateVirtualCameraState('No se pudo comprobar · revisa notificaciones', false, true);
    reportError(error, 'No se pudo comprobar la cámara virtual');
  }
  updateAvailability();
}

async function installVirtualCamera() {
  ui.installVirtualCamera.disabled = true;
  updateVirtualCameraState('Instalando… autoriza el aviso de Windows si aparece');
  try {
    await invoke('install_virtual_camera');
    virtualCameraInstalled = true;
    updateVirtualCameraState('Instalada y lista', true);
  } catch (error) {
    updateVirtualCameraState('Falló la instalación · revisa notificaciones', false, true);
    reportError(error, 'No se pudo instalar la cámara virtual');
  } finally {
    updateAvailability();
  }
}

async function removeVirtualCamera() {
  if (!await requestConfirmation({
    title: 'Quitar cámara virtual',
    message: 'CameraTuner Virtual Camera dejará de aparecer en Discord, Zoom y las demás aplicaciones hasta que vuelvas a instalarla.',
    confirmLabel: 'Quitar cámara'
  })) return;
  ui.removeVirtualCamera.disabled = true;
  updateVirtualCameraState('Quitando cámara virtual…');
  try {
    await invoke('remove_virtual_camera');
    virtualCameraInstalled = false;
    setVirtualOutputState('stopped');
    updateVirtualCameraState('Lista para instalar');
  } catch (error) {
    updateVirtualCameraState('No se pudo quitar · revisa notificaciones', false, true);
    reportError(error, 'No se pudo quitar la cámara virtual');
  } finally {
    updateAvailability();
  }
}

async function startVirtualOutput() {
  if (!selectedCameraId || !virtualCameraInstalled || virtualOutputState !== 'stopped') return;
  const selection = virtualOutputSelection();
  if (!selection) {
    reportError('La cámara no expone un formato de captura utilizable.', 'No se pudo iniciar la salida virtual');
    return;
  }
  const { width, height, inputFormat } = selection;
  setVirtualOutputState('starting');
  updateVirtualCameraState('Abriendo cámara y procesando vídeo…');
  await stopPreview();
  try {
    await invoke('start_virtual_output', {
      options: {
        cameraId: selectedCameraId,
        width,
        height,
        quality: ui.outputQuality.value,
        inputFormat
      }
    });
    setVirtualOutputState('running');
    updateVirtualCameraState(`${width} × ${height} · salida activa`, true);
  } catch (error) {
    setVirtualOutputState('stopped');
    updateVirtualCameraState('No se pudo iniciar · revisa notificaciones', false, true);
    reportError(error, 'No se pudo iniciar la salida virtual');
  }
}

async function stopVirtualOutput() {
  if (virtualOutputState === 'stopped') return;
  setVirtualOutputState('stopping');
  try {
    if (hasTauriRuntime()) await invoke('stop_virtual_output');
    updateVirtualCameraState(virtualCameraInstalled ? 'Instalada y lista' : 'Lista para instalar', virtualCameraInstalled);
  } catch (error) {
    updateVirtualCameraState('No se pudo detener · revisa notificaciones', false, true);
    reportError(error, 'No se pudo detener la salida virtual');
  } finally {
    setVirtualOutputState('stopped');
  }
}

async function checkVirtualOutputHealth() {
  if (!hasTauriRuntime() || virtualOutputState !== 'running' || virtualOutputHealthCheckInFlight) return;
  virtualOutputHealthCheckInFlight = true;
  try {
    const running = await invoke('get_virtual_output_running');
    if (!running && virtualOutputState === 'running') {
      setVirtualOutputState('stopped');
      updateVirtualCameraState('La captura se detuvo · vuelve a activarla', false, true);
      addNotification(
        'El productor nativo dejó de publicar fotogramas. Puedes volver a activar la salida desde CameraTuner.',
        'La salida virtual se detuvo',
        'cámara virtual'
      );
    }
  } catch {
    // invoke already logs the error; the next interval retries the health check.
  } finally {
    virtualOutputHealthCheckInFlight = false;
  }
}

async function toggleVirtualOutput() {
  if (virtualOutputState === 'running') await stopVirtualOutput();
  else if (virtualOutputState === 'stopped') await startVirtualOutput();
}

async function openDiagnostics() {
  try {
    await invoke('open_diagnostics_folder');
  } catch (error) {
    reportError(error, 'No se pudo abrir la carpeta de diagnóstico');
  }
}

initializePanelState();
initializeWorkspaceSplitter();
setTheme((readStorage(themeStorageKey) ?? readStorage(legacyThemeStorageKey)) === 'dark');
ui.theme.addEventListener('change', () => setTheme(ui.theme.checked));
ui.camera.addEventListener('change', () => selectCamera(ui.camera.value));
ui.profile.addEventListener('change', () => enqueueControlOperation(applyProfile)
  .catch((error) => reportError(error, 'No se pudo aplicar el perfil')));
ui.refreshCameras.addEventListener('click', refreshCameras);
ui.restoreDefaults.addEventListener('click', () => enqueueControlOperation(restoreDefaults)
  .catch((error) => reportError(error, 'No se pudieron restablecer los controles')));
ui.driverProperties.addEventListener('click', () => enqueueControlOperation(openDriverProperties));
ui.captureFormat.addEventListener('change', () => void changeCaptureFormat()
  .catch((error) => reportError(error, 'No se pudo cambiar el formato de captura')));
ui.addFilter.addEventListener('click', async () => {
  filterCatalogReturnFocus = document.activeElement;
  try {
    await refreshFilterPlugins();
  } catch (error) {
    reportError(error, 'No se pudo actualizar el catálogo de plugins');
  }
  ui.filterCatalog.showModal();
});
ui.filterCatalog.addEventListener('close', () => {
  if (filterCatalogReturnFocus instanceof HTMLElement && filterCatalogReturnFocus.isConnected) {
    filterCatalogReturnFocus.focus();
  }
  filterCatalogReturnFocus = null;
});
ui.filterPluginFile.addEventListener('change', () => void installFilterPlugin(ui.filterPluginFile.files?.[0]));
ui.saveProfile.addEventListener('click', saveProfile);
ui.deleteProfile.addEventListener('click', () => void deleteProfile());
ui.profileName.addEventListener('input', updateProfileDialog);
ui.profileForm.addEventListener('submit', (event) => {
  event.preventDefault();
  const name = ui.profileName.value.trim().slice(0, 80);
  if (!name) {
    ui.profileName.focus();
    return;
  }
  if (commitProfile(name)) ui.profileDialog.close('saved');
});
ui.profileCancel.addEventListener('click', () => ui.profileDialog.close('cancel'));
ui.profileDialogClose.addEventListener('click', () => ui.profileDialog.close('cancel'));
ui.profileDialog.addEventListener('close', () => {
  if (profileDialogReturnFocus instanceof HTMLElement && profileDialogReturnFocus.isConnected) {
    profileDialogReturnFocus.focus();
  }
  profileDialogReturnFocus = null;
});
ui.previewToggle.addEventListener('click', togglePreview);
ui.probeNativeBackend.addEventListener('click', probeNativeBackend);
ui.installVirtualCamera.addEventListener('click', installVirtualCamera);
ui.removeVirtualCamera.addEventListener('click', removeVirtualCamera);
ui.virtualOutputToggle.addEventListener('click', toggleVirtualOutput);
ui.openDiagnostics.addEventListener('click', openDiagnostics);
ui.notificationOpenDiagnostics.addEventListener('click', openDiagnostics);
ui.notificationToggle.addEventListener('click', (event) => {
  event.stopPropagation();
  toggleNotificationPanel();
});
ui.notificationPanel.addEventListener('click', (event) => event.stopPropagation());
ui.clearNotifications.addEventListener('click', () => {
  notifications = [];
  saveNotifications();
  renderNotifications();
});
document.addEventListener('click', () => toggleNotificationPanel(false));
ui.outputQuality.addEventListener('change', () => void writeDiagnostic('info', 'virtual_output.quality_changed', 'Calidad de reescalado modificada.', { quality: ui.outputQuality.value }));
document.querySelectorAll('select').forEach((select) => {
  select.addEventListener('change', () => syncSelectTitle(select));
  syncSelectTitle(select);
});
document.addEventListener('contextmenu', (event) => {
  if (!event.target.closest('input:not([type="range"]):not([type="checkbox"]):not([type="file"]), textarea, [contenteditable="true"]')) {
    event.preventDefault();
  }
});
document.addEventListener('dragstart', (event) => {
  if (event.target instanceof HTMLImageElement) event.preventDefault();
});
document.addEventListener('keydown', (event) => {
  const key = event.key.toLowerCase();
  const dialogOpen = Boolean(document.querySelector('dialog[open]'));
  if (event.key === 'F5' && !dialogOpen) {
    event.preventDefault();
    if (!ui.refreshCameras.disabled) void refreshCameras();
    return;
  }
  if (event.ctrlKey && !event.altKey && key === 's' && !dialogOpen) {
    event.preventDefault();
    if (!ui.saveProfile.disabled) saveProfile();
    return;
  }
  if (event.ctrlKey && ['+', '-', '=', '0'].includes(key)) event.preventDefault();
  if (event.key === 'Escape' && !dialogOpen) toggleNotificationPanel(false);
});
document.querySelectorAll('dialog').forEach((dialog) => {
  dialog.addEventListener('click', (event) => {
    if (event.target === dialog) dialog.close('cancel');
  });
});
window.addEventListener('error', (event) => {
  void writeDiagnostic('fatal', 'javascript.uncaught_error', event.message || 'Error JavaScript no controlado.', {
    filename: event.filename?.split('/').pop(),
    line: event.lineno,
    column: event.colno,
    stack: event.error?.stack
  });
});
window.addEventListener('unhandledrejection', (event) => {
  void writeDiagnostic('fatal', 'javascript.unhandled_rejection', messageFrom(event.reason), {
    stack: event.reason instanceof Error ? event.reason.stack : undefined
  });
  reportError(event.reason);
});
document.addEventListener('visibilitychange', () => void writeDiagnostic('debug', 'document.visibility_changed', 'Visibilidad de la interfaz actualizada.', { state: document.visibilityState }));

loadNotifications();
renderNotifications();
renderProfiles();
renderControls();
renderFilterGraph();
void writeDiagnostic('info', 'frontend.ready', 'Interfaz cargada y listeners registrados.', {
  userAgent: navigator.userAgent,
  language: navigator.language
});
refreshVirtualCameraStatus();
window.setInterval(() => void checkVirtualOutputHealth(), 2000);
void initializeFilterSystem().finally(() => refreshCameras());
