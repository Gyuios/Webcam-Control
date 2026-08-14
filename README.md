# Webcam-Control / CameraTuner

Aplicación de escritorio abierta para controlar webcams, procesar su imagen y publicarla como cámara virtual en Windows. La interfaz usa Tauri 2; el pipeline principal permanece en procesos nativos y el preview compatible cruza el IPC como JPEG acotado.

## Descargar

Los instaladores para Windows están disponibles en [GitHub Releases](https://github.com/Gyuios/Webcam-Control/releases). Esta es una versión temprana: puede activar una advertencia de SmartScreen porque el ejecutable todavía no tiene firma comercial de código.

Descarga el archivo `CameraTuner_*_x64-setup.exe`, ejecútalo como administrador y reinicia las aplicaciones de videollamada después de instalar o quitar la cámara virtual.

## Funcionalidad actual

- Controles físicos expuestos por el driver mediante `IAMCameraControl` e `IAMVideoProcAmp`, incluidos modos automático/manual cuando existen.
- Acceso serializado al panel original del fabricante para controles propietarios que no pueden normalizarse con seguridad.
- Captura persistente nativa mediante Windows `MediaCapture` + `MediaFrameReader`.
- Selector por cámara de todos los modos nativos anunciados: resolución, FPS y formato de píxel, con elección automática como fallback.
- Preview local con detección de bloqueo y recuperación limpia de la cámara.
- Grafo de filtros digital ordenable con flechas, repetible y con bypass por nodo: brillo, contraste, saturación, gamma, temperatura, matiz y orientación.
- Corrección de lente Brown–Conrady y múltiples LUT 3D `.cube` con mezcla regulable.
- Plugins declarativos seguros con controles personalizados y transformación de color, sin cargar DLL de terceros.
- Perfiles por dispositivo que conservan controles físicos y el grafo digital, con migración desde perfiles v3/v4.
- Reescalado BGRA bilinear o Lanczos3.
- Cámara virtual Media Foundation de espacio de usuario; no instala un driver de kernel ni requiere firma de drivers.
- IPC JSON Lines versionado, leases de dispositivo y logs estructurados locales con rotación.

La opción de reescalado IA permanece deshabilitada hasta integrar y medir un modelo ONNX redistribuible. La aplicación nunca llama “IA” al reescalado clásico.

## Arquitectura

```text
UI Tauri
  ├─ Rust: coordinación, preview JPEG e IPC
  ├─ C#: controles DirectShow del driver (sidecar transitorio)
  └─ camera-host Rust
       └─ MediaCapture / MediaFrameReader
            └─ filtros + LUT + corrección + scaler
                 └─ CTFRAME2 (triple búfer latest-frame)
                      └─ Media Foundation Virtual Camera
```

Empieza por el [contexto técnico integral](PROJECT-CONTEXT.md). Consulta también [ARCHITECTURE.md](docs/ARCHITECTURE.md), la [evaluación de investigación](docs/RESEARCH-EVALUATION.md), el [plan](docs/IMPLEMENTATION-PLAN.md) y el [ADR del host nativo](docs/adr/0001-native-camera-host.md).

## Requisitos

- Windows 11 x64, build 22000 o posterior.
- Node.js 24 LTS y Rust estable MSVC.
- .NET SDK 10 (sidecar de controles).
- Visual Studio 2022 Build Tools, C++ de escritorio y Windows SDK (cámara virtual).
- WebView2 Runtime.

```powershell
npm run doctor
```

Si faltan herramientas, ejecuta `scripts\setup-toolchain.ps1` desde una consola elevada.

## Desarrollo y verificación

```powershell
npm ci
npm run assets
npm run dev
```

`prepare-assets.ps1` compila el host Rust, el sidecar de controles y los componentes Media Foundation. El ejemplo Windows-Camera se descarga desde una revisión inmutable, se verifica por SHA-256 y se parchea de forma reproducible.

```powershell
npm run check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

CI ejecuta las mismas validaciones en Windows y trata warnings como errores.

## Diagnóstico

El botón **Abrir registros de diagnóstico** muestra `webcam-control.jsonl` dentro del directorio local de logs. Rota a 5 MB, conserva cinco archivos y registra estados, IPC, duraciones y errores de backend. Redacta identificadores/rutas de dispositivos y nunca almacena imágenes ni envía telemetría.

## Instalador y licencia

`npm run build` genera un instalador NSIS por máquina. La elevación registra bajo HKLM la Media Source COM y prepara el almacenamiento compartido; la cámara virtual se crea para el usuario actual. La ejecución directa también puede preparar esos requisitos mediante un aviso UAC al pulsar Instalar. Para distribución pública conviene firma normal de código para reducir advertencias de SmartScreen; no es firma de driver.

El proyecto se publica bajo [MIT](LICENSE.txt). Las atribuciones de componentes externos están en [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
