# Evaluación de la investigación técnica

Fecha de evaluación: 2026-08-12
Documento evaluado: `deep-research-report.md`
Proyecto: Webcam-Control (nombre de producto actual: CameraTuner)

## Veredicto

La investigación es sólida y propone la dirección correcta. Su decisión más
importante no es una librería concreta, sino este invariante:

> Una cámara física debe tener exactamente un propietario dentro de la
> aplicación. Captura, controles, páginas del fabricante y recuperación deben
> pasar por ese propietario y por una única máquina de estados.

Tauri no es un obstáculo para esta arquitectura. Debe conservarse como UI y
plano de comandos, manteniendo el vídeo y las APIs de dispositivo en procesos
nativos supervisados por Rust.

No se debe interpretar "aplicar toda la investigación" como incorporar todas
las dependencias nombradas. El propio informe clasifica varias como alternativas
mutuamente excluyentes o tecnologías para fases futuras. La aplicación correcta
es ejecutar los spikes, conservar los fallbacks y adoptar sólo la opción que
supere criterios medibles.

### Resultado de los primeros spikes

`MediaCapture` + `MediaFrameReader` ganó el camino normal: enumeró y capturó la UVC a 640×360, 1280×720 y 1920×1080, entregó BGRA verificable, cerró limpiamente y alimentó preview y salida compartida. SourceReader queda como herramienta diagnóstica. FFmpeg se retiró del runtime y del bundle porque mantener un segundo propietario no utilizado aumentaba peso y superficie de fallos; Git conserva el baseline histórico si hiciera falta comparar una regresión.

## Lo que se adopta como decisión arquitectónica

| Área | Decisión |
|---|---|
| UI | Tauri 2, HTML/CSS/JS. No accede directamente al hardware. |
| Orquestación | Rust: estado, perfiles, PnP, leases, watchdog, IPC y comandos Tauri. |
| Aislamiento | Un proceso `camera-host` por cámara activa; reiniciable y supervisado. |
| Captura primaria | Media Foundation, con formato explícito y superficies D3D11 cuando sean estables. |
| Controles | Capa normalizada que combina WinRT/KS/IAM según soporte real del dispositivo. |
| Property pages | DirectShow, siempre serializadas por el propietario de la cámara. |
| Procesamiento | D3D11 + HLSL; grafo ordenado y con colas acotadas. |
| CPU fallback | libyuv para conversión, escala y rotación de formatos comunes. |
| Cámara virtual | `MFCreateVirtualCamera` + Custom Media Source C++ en user mode. |
| Calibración | OpenCV fuera del bucle normal de vídeo; produce mapas para el shader. |
| FFmpeg | Fallback, diagnóstico y comparación; no propietario normal concurrente. |
| IA | Frontera ONNX; Windows ML/ONNX Runtime se decide mediante benchmarks posteriores. |
| Telemetría | Logs estructurados, métricas de latencia, watchdog y dumps locales. |

## Matices y correcciones al informe

### 1. El spike de captura necesita una cuarta opción nativa

Además de Media Foundation Source Reader y MediaCapture/MediaFrameReader, se
debe medir `IMFCaptureEngine`. Es la API de Media Foundation que controla fuente,
preview y sinks de captura. Source Reader es útil y directo, pero no administra
un presentation clock y Microsoft desaconseja depender de sus conversiones de
software para tiempo real. No conviene fijar Source Reader como ganador antes de
medir recuperación, controles, timestamps y superficies GPU.

Opciones del spike:

1. `MediaCapture` + `MediaFrameReader` + `VideoDeviceController`.
2. `IMFCaptureEngine` + callbacks de muestras.
3. `IMFSourceReader` configurado explícitamente.
4. GStreamer `mfvideosrc`, como alternativa integral.
5. FFmpeg DirectShow actual, como baseline y fallback.

### 2. `IMFCameraControlDefaults` no es una API universal de controles vivos

La familia `IMFCameraControlDefaults*` sirve para describir/aplicar valores y
su momento de configuración pre-start o post-start. No sustituye por sí sola a
`VideoDeviceController`, `IKsControl`, `IAMCameraControl` e
`IAMVideoProcAmp`. La capa del producto debe consultar capacidades reales y
devolver una representación común sin inventar controles inexistentes.

### 3. C# no causó el problema por ser C#

El riesgo actual es que el puente DirectShow puede abrir el mismo dispositivo
que FFmpeg desde otro proceso y otro dominio de exclusión. El puente puede
permanecer durante la migración. Se elimina sólo después de que `camera-host`
alcance paridad en enumeración, controles y property pages.

### 4. `CurrentUser` evita elevación al crear la cámara, no necesariamente al
registrar el servidor COM

La llamada actual usa `MFVirtualCameraAccess_CurrentUser`, por lo que crear la
instancia no necesita un driver ni privilegios de kernel. Sin embargo, el
instalador actual registra la DLL en HKLM y usa instalación `perMachine`, lo que
sí requiere elevación. Debe existir un spike de instalación por usuario antes de
prometer una experiencia completamente sin administrador.

### 5. D3D11 no implica zero-copy automáticamente

Media Foundation puede entregar superficies GPU, pero cada frontera debe
medirse: decodificación MJPEG/H.264, conversión YUY2/NV12, proceso cruzado,
preview y Custom Media Source. El transporte por memoria compartida debe seguir
existiendo como fallback incluso después de introducir texturas compartidas.

### 6. GStreamer es un plan B serio, no una dependencia obligatoria

`mfvideosrc` y los elementos D3D11 justifican un prototipo. No garantizan por sí
solos ownership coordinado, controles completos ni zero-copy de extremo a
extremo. Sólo se adopta si reduce complejidad total después de incluir runtime,
plugins, packaging, diagnóstico y mantenimiento.

### 7. La preview necesita una decisión explícita

La preview actual devuelve JPEG por IPC Tauri y crea `Blob` en JavaScript. Es
mejor que Base64 o un archivo temporal por cuadro, pero el vídeo sí cruza el
WebView. El spike debe comparar:

- preview nativa D3D11/DirectComposition en una superficie asociada al HWND de
  Tauri;
- preview comprimida hacia el WebView como fallback sencillo;
- coste y complejidad de sincronizar layout, DPI, resize y ventanas ocultas.

### 8. "Proceso por dispositivo" significa por cámara activa

No hace falta abrir o mantener un proceso para todas las cámaras enumeradas.
El coordinador inicia un host por cámara que esté siendo usada. La arquitectura
queda preparada para varias cámaras, pero el MVP puede limitarse a una activa.

### 9. La IA no debe bloquear el pipeline convencional

La superresolución de una webcam exige estabilidad temporal además de calidad
por frame. El scheduler conserva sólo el frame más reciente, descarta trabajo
vencido y vuelve automáticamente a Lanczos/bilinear si el runtime, el modelo o
la GPU no cumplen el presupuesto. Los pesos se administran con manifiesto,
versión, hash y licencia separados del código.

## Diferencia con el código actual

| Componente actual | Evaluación | Destino |
|---|---|---|
| Tauri 2 + UI web sin framework | Adecuado y liviano. | Conservar. Añadir contratos tipados y pruebas. |
| `src-tauri/src/main.rs` de más de 1.400 líneas | Funciona, pero mezcla UI IPC, procesos, captura y estados. | Dividir antes de añadir MF/D3D11. |
| Puente C# DirectShow persistente | Útil y actualmente probado. No comparte ownership con FFmpeg. | Fallback transitorio; absorber capacidades en `camera-host`. |
| FFmpeg DirectShow para preview/salida | Buen baseline, amplia compatibilidad. Es el dueño normal actual. | Mover a backend fallback. |
| Preview MJPEG por stdout/IPC | Último frame en memoria y sin Base64: mejora válida. | Fallback mientras se prueba preview nativa. |
| BGRA en `%ProgramData%` | Simple, reiniciable y válido para 720p/1080p. | Mantener como transporte v1; añadir límites y seguridad por usuario. |
| Custom Media Source del sample de Microsoft | Dirección correcta. | Convertir salida nominal a NV12 y robustecer timestamps/fallback slate. |
| Helper `MFCreateVirtualCamera` | Usa CurrentUser/System correctamente. | Conservar; desacoplar estado real de un marcador de registro. |
| Instalador NSIS per-machine/HKLM | Funcional, pero requiere admin. | Medir registro COM por usuario y diseñar upgrade/uninstall seguro. |
| Logs estructurados propios | Base valiosa para reproducir fallos. | Mantener; migrar a `tracing` y añadir IDs de sesión/estado. |

El build local evaluado está sano: 5 pruebas JavaScript, 6 pruebas Rust y la
compilación Release del puente C# pasan sin errores. Esto permite conservar el
backend existente como ruta `legacy` mientras se construye el nuevo host.

## Clasificación de tecnologías

### Adoptar en la siguiente arquitectura

- `windows-rs`.
- Media Foundation y D3D11.
- HLSL propio.
- IAM/KS y property pages como compatibilidad.
- MF virtual camera actual.
- Estado/lease/watchdog/IPC versionado.
- Memoria compartida como fallback.

### Incorporar cuando la fase lo necesite

- libyuv: fallback CPU del motor.
- OpenCV/opencv-rust: calibración y generación de mapas.
- ONNX Runtime o Windows ML: después del benchmark de IA.

### Prototipar antes de decidir

- MediaCapture/MediaFrameReader.
- `IMFCaptureEngine`.
- MF Source Reader.
- GStreamer + `mfvideosrc` + D3D11.
- libdshowcapture como implementación de referencia/fallback.
- texturas D3D11 compartidas entre procesos.
- preview nativa dentro de la ventana Tauri.
- Windows ML frente a ONNX Runtime empaquetado.

### Postergar

- OpenColorIO, Little CMS y libplacebo.
- HDR/10-bit/ACES.
- wgpu y Vulkan.
- ARM64.
- plugins de Extension Units por fabricante.
- superresolución, segmentación y denoise por IA.

### No integrar en el núcleo inicial

- OpenCV como motor de captura o renderer continuo.
- libav* enlazado dentro del host antes de demostrar una necesidad.
- DirectML directo como API de producto nueva.
- múltiples dueños concurrentes del mismo dispositivo.
- identificación por friendly name.
- colas de frames sin límite.
- reseteo PnP automático o cualquier acción administrativa destructiva.

## Resultado de la búsqueda de skills

Se consultó el catálogo oficial disponible y repositorios externos relevantes.
No existe actualmente una skill oficial específica para Tauri, Media Foundation,
D3D11/HLSL o cámaras virtuales.

Skills oficiales recomendadas para instalar cuando comience cada actividad:

| Skill | Uso en este proyecto | Prioridad |
|---|---|---|
| `security-best-practices` | FFI, IPC, parsers `.cube`, permisos y sidecars. | Alta |
| `security-threat-model` | Límites de confianza: WebView, host, Frame Server, modelos y archivos. | Alta |
| `gh-fix-ci` | Diagnosticar y corregir fallos del workflow Windows. | Alta al abrir PRs |
| `playwright` | Pruebas de la UI web aislada con backend simulado. | Media |
| `playwright-interactive` | Depuración manual de estados visuales complejos. | Media/baja |
| `security-ownership-map` | Ownership de componentes cuando haya más contribuidores. | Baja ahora |
| `sentry` | Telemetría remota opcional, sólo con diseño explícito de privacidad. | No instalar ahora |

Skills ya disponibles que sí ayudan:

- GitHub: inspección de repositorios, issues, CI y PRs.
- Computer Use: pruebas reales de la aplicación y aplicaciones consumidoras.
- Screenshot: evidencia visual de errores y regresiones.

Candidatos externos evaluados:

- `leonardomso/rust-skills`: MIT, útil como guía de Rust idiomático y seguro.
- Trail of Bits `rust-review`: revisión profunda de Rust/FFI/concurrencia; es un
  plugin orquestado con scripts y agentes propios, no conviene instalarlo como
  una única skill suelta sin validar compatibilidad del paquete completo.
- Sentry `security-review`: revisión general útil, pero solapa bastante con las
  dos skills oficiales de seguridad.

No se instalaron automáticamente: la búsqueda de skills debe separar selección
de instalación, y las externas requieren revisar su paquete completo antes de
darles acceso al repositorio.

## Fuentes primarias verificadas

- [MFCreateVirtualCamera](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera)
- [IMFCaptureEngine](https://learn.microsoft.com/en-us/windows/win32/api/mfcaptureengine/nn-mfcaptureengine-imfcaptureengine)
- [MediaFrameReader](https://learn.microsoft.com/en-us/windows/apps/develop/camera/process-media-frames-with-mediaframereader)
- [VideoDeviceController](https://learn.microsoft.com/en-us/uwp/api/windows.media.devices.videodevicecontroller)
- [Manual camera controls](https://learn.microsoft.com/en-us/windows/apps/develop/camera/capture-device-controls-for-photo-and-video-capture)
- [Windows-Camera sample](https://github.com/microsoft/Windows-Camera)
- [windows-rs](https://github.com/microsoft/windows-rs)
- [libdshowcapture](https://github.com/obsproject/libdshowcapture)
- [GStreamer mfvideosrc](https://gstreamer.freedesktop.org/documentation/mediafoundation/mfvideosrc.html)
- [Tauri sidecars](https://v2.tauri.app/develop/sidecar/)
- [Windows ML execution providers](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/supported-execution-providers)
