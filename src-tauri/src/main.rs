#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State, WindowEvent,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

struct PreviewState {
    process: Mutex<Option<Child>>,
    image_path: Mutex<Option<PathBuf>>,
    log_path: Mutex<Option<PathBuf>>,
}

impl PreviewState {
    fn new() -> Self {
        Self {
            process: Mutex::new(None),
            image_path: Mutex::new(None),
            log_path: Mutex::new(None),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Camera {
    id: String,
    name: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CameraControl {
    id: String,
    name: String,
    kind: String,
    property: i32,
    minimum: i32,
    maximum: i32,
    step: i32,
    default_value: i32,
    value: i32,
    automatic: bool,
    supports_auto: bool,
}

#[derive(Deserialize)]
struct BridgeResult {
    ok: bool,
    error: Option<String>,
}

fn binary_path(app: &tauri::AppHandle, name: &str) -> Result<PathBuf, String> {
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries").join(name);
    if development.exists() {
        return Ok(development);
    }
    app.path()
        .resource_dir()
        .map(|folder| folder.join("binaries").join(name))
        .map_err(|error| format!("No se pudo localizar un componente de Control Webcam: {error}"))
}

fn bridge_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    binary_path(app, "control-webcam-bridge-x86_64-pc-windows-msvc.exe")
}

fn ffmpeg_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    binary_path(app, "ffmpeg.exe")
}

fn bridge(app: &tauri::AppHandle, args: &[String]) -> Result<String, String> {
    let output = Command::new(bridge_path(app)?)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("No se pudo iniciar el motor de cámara: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        return Err("El motor de cámara no devolvió información.".to_string());
    }
    if output.status.success() {
        Ok(text)
    } else {
        let message = serde_json::from_str::<BridgeResult>(&text)
            .ok()
            .and_then(|result| if result.ok { None } else { result.error })
            .unwrap_or(text);
        Err(message)
    }
}

fn get_camera_list(app: &tauri::AppHandle) -> Result<Vec<Camera>, String> {
    serde_json::from_str(&bridge(app, &["list".into()])?).map_err(|error| error.to_string())
}

fn stop_preview_process(state: &PreviewState) {
    if let Ok(mut process) = state.process.lock() {
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    if let Ok(mut path) = state.image_path.lock() {
        *path = None;
    }
    if let Ok(mut path) = state.log_path.lock() {
        *path = None;
    }
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn list_cameras(app: tauri::AppHandle) -> Result<Vec<Camera>, String> {
    get_camera_list(&app)
}

#[tauri::command]
fn get_controls(app: tauri::AppHandle, camera_id: String) -> Result<Vec<CameraControl>, String> {
    serde_json::from_str(&bridge(&app, &["controls".into(), camera_id])?).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_preferences(preferences: serde_json::Value) -> Result<(), String> {
    let _ = preferences;
    Ok(())
}

#[tauri::command]
fn set_control(app: tauri::AppHandle, camera_id: String, kind: String, property: i32, value: i32, automatic: bool) -> Result<(), String> {
    let output = bridge(&app, &[
        "set".into(), camera_id, kind, property.to_string(), value.to_string(), automatic.to_string(),
    ])?;
    let result: BridgeResult = serde_json::from_str(&output).map_err(|error| error.to_string())?;
    if result.ok {
        Ok(())
    } else {
        Err(result.error.unwrap_or_else(|| "El controlador rechazó el cambio.".to_string()))
    }
}

#[tauri::command]
fn start_preview(app: tauri::AppHandle, state: State<PreviewState>, camera_id: String) -> Result<(), String> {
    stop_preview_process(&state);
    let camera = get_camera_list(&app)?
        .into_iter()
        .find(|item| item.id == camera_id)
        .ok_or_else(|| "La cámara seleccionada ya no está disponible.".to_string())?;

    let cache_dir = app.path()
        .app_cache_dir()
        .map_err(|error| format!("No se pudo preparar la vista previa: {error}"))?
        .join("preview");
    fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("No se pudo preparar la carpeta de vista previa: {error}"))?;
    let image_path = cache_dir.join("current-frame.jpg");
    let log_path = cache_dir.join("ffmpeg-preview.log");
    // Command::args conserva cada elemento como un único argumento; a diferencia
    // de una consola, no hay que añadir comillas para los nombres con espacios.
    let input = format!("video={}", camera.name.replace('"', "'"));
    let output = image_path.to_string_lossy().to_string();

    let stderr = fs::File::create(&log_path)
        .map_err(|error| format!("No se pudo crear el registro de vista previa: {error}"))?;
    let mut child = Command::new(ffmpeg_path(&app)?)
        .args([
            "-hide_banner", "-loglevel", "error", "-f", "dshow", "-rtbufsize", "256M",
            "-i", &input, "-an", "-vf", "scale=960:-2:flags=fast_bilinear", "-q:v", "6",
            "-f", "image2", "-update", "1", "-atomic_writing", "1", "-y", &output,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| format!("No se pudo iniciar la captura de vídeo: {error}"))?;

    thread::sleep(Duration::from_millis(450));
    if let Some(status) = child.try_wait().map_err(|error| format!("No se pudo comprobar la vista previa: {error}"))? {
        let detail = fs::read_to_string(&log_path).unwrap_or_default();
        return Err(format!("FFmpeg no pudo abrir la cámara ({status}). {}", detail.trim()));
    }

    *state.process.lock().map_err(|_| "No se pudo iniciar la vista previa.".to_string())? = Some(child);
    *state.image_path.lock().map_err(|_| "No se pudo iniciar la vista previa.".to_string())? = Some(image_path);
    *state.log_path.lock().map_err(|_| "No se pudo iniciar la vista previa.".to_string())? = Some(log_path);
    Ok(())
}

#[tauri::command]
fn get_preview_frame(state: State<PreviewState>) -> Result<Option<String>, String> {
    if let Ok(mut process) = state.process.lock() {
        if let Some(child) = process.as_mut() {
            if let Some(status) = child.try_wait().map_err(|error| format!("No se pudo comprobar la vista previa: {error}"))? {
                *process = None;
                let detail = state.log_path.lock().ok().and_then(|path| path.clone())
                    .and_then(|path| fs::read_to_string(path).ok())
                    .unwrap_or_default();
                return Err(format!("La captura se detuvo ({status}). {}", detail.trim()));
            }
        }
    }
    let path = state.image_path.lock()
        .map_err(|_| "No se pudo leer la vista previa.".to_string())?
        .clone();
    let Some(path) = path else { return Ok(None); };
    match fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => Ok(Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes))),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("No se pudo leer el vídeo: {error}")),
    }
}

#[tauri::command]
fn stop_preview(state: State<PreviewState>) -> Result<(), String> {
    stop_preview_process(&state);
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(PreviewState::new())
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "Mostrar Control Webcam", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let icon = app.default_window_icon().cloned().ok_or("No se encontró el icono de la aplicación.")?;
            TrayIconBuilder::with_id("control-webcam-tray")
                .icon(icon)
                .tooltip("Control Webcam")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => {
                        stop_preview_process(&app.state::<PreviewState>());
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event {
                        show_main_window(&tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => {
                let _ = window.hide();
            }
            WindowEvent::CloseRequested { .. } => {
                stop_preview_process(&window.app_handle().state::<PreviewState>());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            list_cameras,
            get_controls,
            save_preferences,
            set_control,
            start_preview,
            get_preview_frame,
            stop_preview
        ])
        .run(tauri::generate_context!())
        .expect("No se pudo iniciar Control Webcam");
}
