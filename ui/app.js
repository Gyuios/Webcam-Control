let controls = [];
let selectedCameraId = '';
let previewActive = false;
let previewTimer = null;
const scheduledControls = new Map();
const profileStorageKey = 'control-webcam-profiles-v2';

const ui = {
  controls: document.querySelector('#control-list'),
  theme: document.querySelector('#theme-toggle'),
  camera: document.querySelector('#camera-select'),
  profile: document.querySelector('#profile-select'),
  state: document.querySelector('#camera-state'),
  previewToggle: document.querySelector('#preview-toggle'),
  previewEmpty: document.querySelector('#preview-empty'),
  previewImage: document.querySelector('#preview-image'),
  previewStage: document.querySelector('#preview-stage'),
  previewStatus: document.querySelector('#preview-status')
};

function setTheme(isDark) {
  document.documentElement.dataset.theme = isDark ? 'dark' : 'light';
  ui.theme.checked = isDark;
  localStorage.setItem('control-webcam-theme', isDark ? 'dark' : 'light');
}

function updateCameraState(text, online = false) {
  ui.state.lastChild.textContent = ` ${text}`;
  ui.state.classList.toggle('online', online);
}

function paintSlider(slider) {
  const min = Number(slider.min);
  const max = Number(slider.max);
  const percentage = ((Number(slider.value) - min) / (max - min)) * 100;
  slider.style.setProperty('--progress', `${percentage}%`);
}

function renderControls() {
  const template = document.querySelector('#control-template');
  ui.controls.replaceChildren();
  if (!controls.length) {
    ui.controls.innerHTML = '<p class="empty-controls">Selecciona una cámara para cargar sus ajustes.</p>';
    return;
  }

  controls.forEach((control) => {
    const row = template.content.firstElementChild.cloneNode(true);
    const [name, range] = row.querySelectorAll('.control-name *');
    const slider = row.querySelector('.slider');
    const value = row.querySelector('.value-box');
    const auto = row.querySelector('.auto-button');
    name.textContent = control.name;
    range.textContent = `${control.minimum} — ${control.maximum}`;
    slider.min = control.minimum;
    slider.max = control.maximum;
    slider.step = Math.max(1, control.step);
    slider.value = control.value;
    paintSlider(slider);
    value.textContent = control.value;
    auto.hidden = !control.supportsAuto;
    auto.classList.toggle('selected', Boolean(control.automatic));
    auto.setAttribute('aria-pressed', String(Boolean(control.automatic)));

    slider.addEventListener('input', () => {
      control.value = Number(slider.value);
      control.automatic = false;
      paintSlider(slider);
      value.textContent = control.value;
      auto.classList.remove('selected');
      auto.setAttribute('aria-pressed', 'false');
      scheduleControl(control, 140);
    });
    slider.addEventListener('change', () => scheduleControl(control, 0));
    auto.addEventListener('click', async () => {
      cancelScheduledControl(control);
      control.automatic = !control.automatic;
      auto.classList.toggle('selected', control.automatic);
      auto.setAttribute('aria-pressed', String(control.automatic));
      await applyControl(control);
    });
    row.querySelector('.reset-button').addEventListener('click', async () => {
      cancelScheduledControl(control);
      control.value = control.initial;
      control.automatic = control.initialAutomatic;
      await applyControl(control);
      renderControls();
    });
    ui.controls.append(row);
  });
}

async function invoke(command, args = {}) {
  if (!window.__TAURI__?.core?.invoke) return undefined;
  try {
    return await window.__TAURI__.core.invoke(command, args);
  } catch (error) {
    updateCameraState(error?.toString?.() || 'El motor de cámara no respondió');
    return undefined;
  }
}

async function applyControl(control) {
  if (!selectedCameraId) return;
  const response = await invoke('set_control', {
    cameraId: selectedCameraId,
    kind: control.kind,
    property: control.property,
    value: control.value,
    automatic: control.automatic
  });
  if (response !== undefined) updateCameraState('Cambios aplicados', true);
}

function scheduleControl(control, delay) {
  cancelScheduledControl(control);
  scheduledControls.set(control.id, window.setTimeout(async () => {
    scheduledControls.delete(control.id);
    await applyControl(control);
  }, delay));
}

function cancelScheduledControl(control) {
  const scheduled = scheduledControls.get(control.id);
  if (scheduled) window.clearTimeout(scheduled);
  scheduledControls.delete(control.id);
}

async function loadControls() {
  if (!selectedCameraId) {
    controls = [];
    renderControls();
    return;
  }
  updateCameraState('Leyendo ajustes del controlador…');
  const response = await invoke('get_controls', { cameraId: selectedCameraId });
  if (!Array.isArray(response)) return;
  controls = response.map((control) => ({
    ...control,
    initial: control.value,
    initialAutomatic: control.automatic
  }));
  renderControls();
  updateCameraState(`${controls.length} ajustes disponibles`, true);
}

async function refreshCameras() {
  updateCameraState('Buscando cámaras…');
  const cameras = await invoke('list_cameras');
  ui.camera.replaceChildren();
  if (!Array.isArray(cameras)) {
    ui.camera.add(new Option('Ejecuta la aplicación Tauri para conectar la cámara', ''));
    updateCameraState('Modo de diseño');
    return;
  }
  if (cameras.length === 0) {
    ui.camera.add(new Option('No se detectaron cámaras', ''));
    selectedCameraId = '';
    await loadControls();
    updateCameraState('Sin cámara detectada');
    return;
  }
  cameras.forEach((camera) => ui.camera.add(new Option(camera.name, camera.id)));
  selectedCameraId = ui.camera.value;
  await loadControls();
}

function readProfiles() {
  try { return JSON.parse(localStorage.getItem(profileStorageKey) || '{}'); }
  catch { return {}; }
}

function writeProfiles(profiles) {
  localStorage.setItem(profileStorageKey, JSON.stringify(profiles));
}

function renderProfiles() {
  const profiles = readProfiles();
  const current = ui.profile.value;
  ui.profile.replaceChildren(new Option('Sin perfil seleccionado', ''));
  Object.keys(profiles).sort().forEach((name) => ui.profile.add(new Option(name, name)));
  ui.profile.value = profiles[current] ? current : '';
}

function saveProfile() {
  if (!controls.length) return;
  const name = window.prompt('Nombre del perfil:');
  if (!name?.trim()) return;
  const profiles = readProfiles();
  profiles[name.trim()] = controls.map(({ id, value, automatic }) => ({ id, value, automatic }));
  writeProfiles(profiles);
  renderProfiles();
  ui.profile.value = name.trim();
}

async function applyProfile() {
  const profile = readProfiles()[ui.profile.value];
  if (!Array.isArray(profile)) return;
  for (const saved of profile) {
    const control = controls.find((item) => item.id === saved.id);
    if (!control) continue;
    control.value = Math.min(control.maximum, Math.max(control.minimum, saved.value));
    control.automatic = Boolean(saved.automatic && control.supportsAuto);
    await applyControl(control);
  }
  renderControls();
}

function deleteProfile() {
  const name = ui.profile.value;
  if (!name) return;
  const profiles = readProfiles();
  delete profiles[name];
  writeProfiles(profiles);
  renderProfiles();
}

async function restoreDefaults() {
  for (const control of controls) {
    cancelScheduledControl(control);
    control.value = control.defaultValue;
    control.automatic = Boolean(control.supportsAuto);
    await applyControl(control);
  }
  renderControls();
}

async function togglePreview() {
  if (!selectedCameraId) return;
  if (previewActive) {
    await stopPreview();
    return;
  }
  const response = await invoke('start_preview', { cameraId: selectedCameraId });
  if (response === undefined) return;
  previewActive = true;
  ui.previewStage.classList.toggle('active', previewActive);
  ui.previewEmpty.hidden = previewActive;
  ui.previewToggle.textContent = 'Detener vista previa';
  ui.previewStatus.textContent = 'Conectando cámara…';
  previewTimer = window.setTimeout(refreshPreviewFrame, 120);
}

async function refreshPreviewFrame() {
  if (!previewActive || !window.__TAURI__?.core?.invoke) return;
  try {
    const frame = await window.__TAURI__.core.invoke('get_preview_frame');
    if (typeof frame !== 'string' || frame.length === 0) return;
    ui.previewImage.src = `data:image/jpeg;base64,${frame}`;
    ui.previewImage.hidden = false;
    ui.previewStatus.textContent = 'En directo';
  } catch (error) {
    ui.previewStatus.textContent = error?.toString?.() || 'Esperando señal de la cámara…';
    if (ui.previewStatus.textContent.includes('se detuvo')) await stopPreview();
  } finally {
    if (previewActive) previewTimer = window.setTimeout(refreshPreviewFrame, 65);
  }
}

async function stopPreview() {
  if (!previewActive) return;
  await invoke('stop_preview');
  previewActive = false;
  if (previewTimer) window.clearTimeout(previewTimer);
  previewTimer = null;
  ui.previewImage.removeAttribute('src');
  ui.previewImage.hidden = true;
  ui.previewStage.classList.remove('active');
  ui.previewEmpty.hidden = false;
  ui.previewToggle.textContent = 'Iniciar vista previa';
  ui.previewStatus.textContent = 'Sin transmisión';
}

setTheme(localStorage.getItem('control-webcam-theme') === 'dark');
ui.theme.addEventListener('change', () => setTheme(ui.theme.checked));
ui.camera.addEventListener('change', async () => { await stopPreview(); selectedCameraId = ui.camera.value; await loadControls(); });
ui.profile.addEventListener('change', applyProfile);
document.querySelector('#refresh-cameras').addEventListener('click', refreshCameras);
document.querySelector('#restore-defaults').addEventListener('click', restoreDefaults);
document.querySelector('#save-profile').addEventListener('click', saveProfile);
document.querySelector('#delete-profile').addEventListener('click', deleteProfile);
ui.previewToggle.addEventListener('click', togglePreview);
renderProfiles();
renderControls();
refreshCameras();
