# Plan de implementación

Este plan migra Webcam-Control desde el backend funcional actual hacia un
pipeline nativo estable. Tauri es el único requisito tecnológico fijo. Cada fase
mantiene una ruta ejecutable y tiene criterios de salida verificables.

## Avance al 2026-08-13

Completado: workspace y contratos, coordinador/leases, host persistente, inventario MF, probes SourceReader y MediaFrameReader, selección de `MediaCapture`, transporte `CTFRAME2` BGRA/NV12 con tres slots y heartbeat, conversión libyuv SIMD, mapping NV12 de capacidad 4K fija en la Media Source v10, fast path NV12, cadencia de salida regulada, preview nativa, grafo CPU ordenable/repetible, corrección Brown–Conrady, múltiples LUT `.cube`, plugins declarativos v1, bilinear/Lanczos3, perfiles v5, productor de cámara virtual, métricas agregadas, logs, tests y CI. FFmpeg fue retirado del runtime y del instalador tras inventariar ambos dispositivos disponibles y validar captura, procesamiento y cierre reales con la UVC.

Pendiente: fast path D3D11/HLSL, calibrador OpenCV, paridad de controles dentro de `camera-host`, hotplug/sleep-resume, soak tests prolongados y benchmark/selección del modelo ONNX. Estas tareas no bloquean el preview ni el procesamiento CPU actuales.

## Principios no negociables

1. Un solo propietario por cámara física.
2. Ninguna operación bloqueante de hardware en el hilo de UI.
3. Los formatos se enumeran y seleccionan explícitamente.
4. Identidad estable por symbolic link/device path; el nombre es sólo etiqueta.
5. Último frame gana; no existen colas de vídeo sin límite.
6. El proceso dueño de la cámara es reiniciable.
7. La cámara virtual sigue produciendo cadence aunque el productor falle.
8. Toda optimización GPU tiene fallback CPU medido.
9. Las funciones físicas y los filtros digitales se muestran por separado.
10. Ninguna librería se adopta sólo porque figure en la investigación.

## Arquitectura objetivo

```text
Tauri WebView
  │ comandos/eventos tipados
  ▼
Rust app/coordinator
  ├─ device registry + profiles
  ├─ lease/state machine
  ├─ watchdog + diagnostics
  └─ IPC versionado
         │
         ▼
camera-host.exe (uno por cámara activa)
  ├─ Media Foundation capture
  ├─ WinRT/KS/IAM controls
  ├─ DirectShow property pages
  ├─ D3D11/HLSL processing (objetivo; CPU actual)
  └─ MediaCapture/MediaFrameReader
         │
         ├─ preview nativa o preview comprimida fallback
         └─ frame transport v2
                  │
                  ▼
          MF Custom Media Source C++
                  │
                  ▼
          MFCreateVirtualCamera
```

## Organización propuesta del repositorio

```text
ui/
src-tauri/
  src/
    app.rs
    commands/
    coordinator/
    diagnostics/
    profiles/
crates/
  camera-protocol/
  camera-domain/
  camera-host/
  windows-camera/
  video-engine/
native/
  virtual-camera/
shaders/
  common/
  filters/
tests/
  contract/
  hardware/
  fixtures/
docs/
  adr/
models/
  manifests/
```

`camera-protocol` no contiene handles COM/D3D ni detalles de una API. Define
mensajes versionados, errores, estados, capacidades, formatos, controles y
métricas. `camera-domain` contiene la máquina de estados y puede probarse sin
hardware. `windows-camera` encapsula `windows-rs` y los bloques `unsafe`/FFI.

## Fase 0 — Congelar una línea base reproducible

Objetivo: poder comparar el backend nuevo sin perder la ruta que hoy funciona.

Trabajo:

- etiquetar internamente el backend actual como `legacy-ffmpeg`;
- documentar UVC Camera y la capturadora Pyle como dos clases de dispositivo
  distintas en la matriz inicial;
- capturar por dispositivo: symbolic link, formatos, FPS, controles, rangos,
  defaults y modo auto/manual;
- registrar time-to-first-frame, frames recibidos/descartados, último frame,
  reinicios, HRESULT/Win32 error y transiciones de estado;
- añadir un exportador de paquete de diagnóstico con redacción de IDs;
- mantener los 5 tests JS, 6 tests Rust y build C# actualmente verdes;
- fijar un conjunto pequeño de vídeos/imágenes golden para color y escala;
- crear feature flags o selección de backend para comparar sin reescribir UI.

Criterios de salida:

- un comando genera el inventario de ambas cámaras y sus modos;
- abrir/cerrar preview 100 veces no deja procesos ni handles conocidos;
- un fallo de FFmpeg vuelve a `Idle/Error` y permite reintentar sin reiniciar la
  aplicación;
- las pruebas existentes siguen pasando.

## Fase 1 — Separar dominio, IPC y procesos sin cambiar el vídeo

Objetivo: reducir el monolito Rust antes de introducir COM/D3D11.

Trabajo:

- dividir `main.rs` en comandos, coordinator, procesos, preview, virtual camera y
  diagnostics;
- crear `camera-domain` con estados explícitos:

```text
Absent -> Idle -> Opening -> Streaming -> Reconfiguring -> Closing -> Idle
                    \-> Busy / PrivacyDenied / DeviceLost / Faulted
```

- implementar lease por ID estable de dispositivo;
- definir protocolo con `protocolVersion`, `requestId`, deadline, cancelación y
  códigos de error tipados;
- sustituir stdout JSON Lines de larga vida por named pipe local una vez que las
  pruebas de protocolo estén listas;
- limitar el pipe al usuario actual y validar tamaño de mensajes;
- supervisar PID, heartbeat y cierre por job object de Windows;
- hacer que Rust sea el único que inicia helpers; el WebView sólo invoca comandos
  allowlisted;
- añadir contract tests y simulador de host fallando/lento/desconectado.

Criterios de salida:

- ningún estado de UI depende de inferir si un proceso existe;
- matar el helper durante una operación no bloquea la app;
- respuestas viejas o de otra selección se descartan;
- no hay dos leases simultáneos para el mismo ID.

## Fase 2 — Spike del propietario nativo de cámara

Objetivo: elegir el backend por evidencia sobre el mismo hardware.

Implementar prototipos mínimos de:

1. MediaCapture + MediaFrameReader.
2. IMFCaptureEngine.
3. IMFSourceReader.
4. GStreamer `mfvideosrc`.
5. Baseline FFmpeg DirectShow actual.

Todos deben usar exactamente el mismo contrato de prueba:

- enumeración de symbolic link y media types;
- selección explícita de NV12, YUY2, MJPEG y resoluciones/FPS disponibles;
- primer frame, timestamps y pérdida de frames;
- memoria CPU frente a superficie D3D11;
- busy/exclusive/privacy;
- unplug/replug, sleep/resume y cierre forzado;
- lectura/escritura de controles durante streaming;
- property page bajo política pause/reopen;
- CPU, GPU, working set, tamaño de distribución y complejidad de código.

Añadir `VideoDeviceController`, `IKsControl`, `IAMCameraControl` e
`IAMVideoProcAmp` dentro del mismo proceso owner. Cada control normalizado guarda:

```text
id, label, source, value type, range, step, default,
manual/auto support, current mode, pre-start/post-start,
readback support, volatile/external-change flag
```

Regla de property pages:

```text
owner lease -> pause/stop if required -> full teardown barrier
-> open page on dedicated COM thread -> close/release
-> enumerate controls/formats again -> reopen -> verify first frame
```

Matriz de decisión sugerida:

| Criterio | Peso |
|---|---:|
| Recuperación y estabilidad | 30% |
| Compatibilidad con controles/driver | 20% |
| D3D11 y número de copias | 15% |
| Latencia/cadence | 15% |
| Complejidad y diagnosticabilidad | 10% |
| Packaging/tamaño | 5% |
| CPU/GPU/energía | 5% |

Criterios de salida:

- 8 horas de captura sin freeze en cada clase de dispositivo;
- 1.000 cambios válidos de controles sin pérdida permanente;
- 100 ciclos de hotplug por clase;
- error busy/privacy correctamente clasificado y recuperable;
- ADR escrita con resultados y backend elegido;
- FFmpeg permanece disponible aunque no gane.

## Fase 3 — Camera host de producción y motor D3D11

Objetivo: reemplazar FFmpeg en el camino normal y procesar sin roundtrip CPU
cuando el dispositivo/driver lo permita.

Trabajo:

- convertir el prototipo ganador en `camera-host`;
- un único hilo/apartment dueño de las operaciones de cada dispositivo;
- D3D11 device manager y pool acotado de texturas;
- decode/conversión con Media Foundation/D3D11 Video Processor según formato;
- representación lineal FP16 para el tramo de color cuando sea necesario;
- grafo HLSL con recursos preasignados y actualización atómica de parámetros;
- orden inicial:

```text
orientation/crop
-> lens map (bypass inicialmente)
-> white balance/temp/tint
-> exposure/gain digital
-> contrast/curves
-> saturation/hue
-> sharpen/blur/denoise básico
-> LUT
-> overlays/compositor
-> output transform NV12
```

- fallback libyuv para equipos/rutas sin superficie GPU;
- métricas por nodo: p50/p95/p99, queue depth, dropped, age del frame;
- shader compile offline; nunca compilar HLSL en cada arranque normal.

Preview:

- spike A: superficie nativa DirectComposition/D3D11 asociada al HWND Tauri;
- spike B: último frame comprimido hacia WebView;
- elegir por estabilidad de resize/DPI/minimize y coste, manteniendo B de fallback.

Criterios de salida:

- 720p30 y 1080p30 sostenidos;
- 1080p60 cuando el dispositivo lo expone y el presupuesto lo permite;
- p95 del procesamiento menor al intervalo objetivo;
- cola máxima de un frame pendiente por etapa interactiva;
- pérdida de D3D device produce recreación o fallback, no cierre de Tauri.

## Fase 4 — Cámara virtual v2

Objetivo: publicar una salida estable y compatible, independiente del estado de
la UI.

Trabajo:

- conservar `MFCreateVirtualCamera` y el Media Source C++;
- publicar inicialmente NV12 1280x720@30 y 1920x1080@30;
- generar timestamps monotónicos a cadence fija;
- usar semántica latest-frame, sin bloquear Frame Server esperando al host;
- si no hay productor: último frame por un timeout corto y luego slate negro o
  imagen de estado configurable;
- diseñar `frame transport v2`:
  - camino nominal: shared D3D11 texture + handle por usuario + keyed mutex o
    sincronización equivalente;
  - fallback: memoria compartida versionada, con header, dimensiones, stride,
    pixel format, sequence, timestamp, active y checksum/commit marker;
- eliminar el archivo global escribible por todos como camino final; usar objeto
  de kernel/ACL del usuario cuando sea posible;
- comprobar consumidores múltiples;
- separar el estado real de MF del simple install marker;
- spike de COM registration per-user; si no es fiable, documentar que sólo la
  instalación requiere elevación.

Criterios de salida:

- Discord, Zoom, Teams, Chrome, Edge, Firefox y OBS reciben vídeo;
- 2–4 consumidores simultáneos no alteran cadence;
- el host y Tauri pueden reiniciarse sin reinstalar la cámara;
- desinstalación quita primero la instancia y después el COM server;
- ningún bloqueo dentro del Media Source depende de IPC vivo.

## Fase 5 — Controles, filtros y perfiles de producto

Objetivo: completar la experiencia principal antes de IA.

Separar visualmente:

- **Cámara/driver:** exposición, foco, white balance, gain, zoom y todo lo que el
  hardware declare de forma segura.
- **Filtros digitales:** brillo/exposición digital, contraste, saturación, hue,
  gamma, temp/tint, mirror, rotate, crop, sharpen y blur.

Trabajo:

- readback después de cada set; si el driver clampa, la UI muestra el valor real;
- debounce/coalescing de sliders, sin cola de cientos de escrituras;
- refresco por eventos/monitor cuando exista y polling limitado como fallback;
- botón de panel original del fabricante;
- perfiles por ID físico + modo de vídeo, no por nombre;
- undo/redo sólo para filtros digitales; los controles físicos guardan snapshot
  y restauración best-effort;
- bypass por filtro y bypass global de procesamiento;
- perfiles de recuperación ante cambio externo o reconexión.

Criterios de salida:

- controles disponibles coinciden con lo expuesto por cada cámara;
- valores auto/manual no se mezclan;
- mover sliders rápido no bloquea ni aumenta latencia de vídeo;
- perfiles no migran accidentalmente entre Pyle y UVC Camera.

## Fase 6 — Corrección óptica, curvas y LUT

Objetivo: añadir las funciones visuales avanzadas más útiles sin convertir
OpenCV en el motor de vídeo.

Trabajo:

- herramienta guiada ChArUco y opción ajedrez;
- calibración Brown-Conrady y modelo fisheye separados;
- perfil por cámara, resolución, crop y modo de lente;
- OpenCV genera mapas UV y métricas de reproyección;
- runtime HLSL aplica el mapa, con controles de crop/balance/FOV;
- importador `.cube` 1D/3D con límites estrictos, parser probado y LUT identidad;
- curvas RGB/luma y scopes opcionales;
- malformed-input tests y golden images contra `cv::remap`.

Criterios de salida:

- calibración repetible por un usuario no técnico;
- error shader frente a referencia cuantificado;
- cambio de resolución no reutiliza una calibración incompatible;
- LUT identidad permanece dentro de tolerancia definida.

## Fase 7 — Laboratorio de IA y reescalado

Objetivo: incorporar IA sólo si ofrece una mejora visible bajo latencia de webcam.

Arquitectura:

```text
latest input frame -> preprocess GPU -> inference slot (máximo 1)
-> temporal/quality checks -> compose -> virtual camera
                         \-> fallback scaler inmediato
```

Trabajo:

- interfaz `AiBackend` independiente del runtime concreto;
- comparar Windows ML administrado con ONNX Runtime empaquetado;
- CPU y GPU de Intel/AMD/NVIDIA; NPU sólo si aporta valor real;
- perfiles x1.5/x2, no asumir que 4x es viable en tiempo real;
- evaluar 720->1080, 1080->1440 y 1080->4K;
- medir time-to-first-frame, steady-state, VRAM/RAM, p95/p99 y calidad temporal;
- evaluar caras, cabello, texto, ruido, movimiento y flicker;
- manifiesto por modelo: nombre, versión, URL/origen, licencia, SHA-256, opset,
  shape, color/range y runtime mínimo;
- no descargar ni ejecutar modelos no verificados;
- fallback automático si inference supera el deadline o falla.

Gate de producto:

- IA no entra en el build estable si añade stutter temporal apreciable;
- nunca acumula más de un frame pendiente;
- desactivar IA no requiere reiniciar el dispositivo;
- el pipeline convencional funciona sin instalar ningún modelo.

## Fase 8 — Hardening y distribución estable

Pruebas obligatorias:

- 24 horas de captura;
- 100+ hotplug por clase de dispositivo;
- sleep/resume, USB hub removal y cambio de puerto;
- 1.000 ciclos open/start/control/stop/reopen;
- 100 ciclos de property page;
- cámara ocupada por otra aplicación y liberación posterior;
- privacy denied;
- kill/restart del host;
- D3D device removed/reset;
- upgrade, downgrade y uninstall del instalador;
- consumidores múltiples de virtual camera;
- fuzz de rangos publicados, protocolo IPC y parsers de perfiles/LUT/modelos.

Ingeniería:

- `cargo fmt`, Clippy con warnings como error y tests por workspace;
- sanitizers/ASan donde sea viable para C++ y fuzzers de parsers;
- revisión de cada bloque `unsafe` y frontera COM/FFI;
- manifests y hashes de todos los sidecars;
- SBOM y avisos de terceros automatizados;
- firma de instalador/binarios cuando haya certificado;
- crash dumps locales con consentimiento y política de retención;
- ningún log contiene symbolic links completos o datos privados sin redacción.

## Orden concreto de los próximos cambios

1. Crear crates `camera-protocol` y `camera-domain`.
2. Extraer estados/procesos/comandos del `main.rs` sin cambiar comportamiento.
3. Añadir backend selector y simulador de host.
4. Implementar device identity + inventario de formatos/controles.
5. Construir el spike MediaCapture/MediaFrameReader.
6. Construir el spike IMFCaptureEngine.
7. Construir el spike Source Reader.
8. Medir GStreamer y baseline FFmpeg.
9. Escribir ADR y escoger backend.
10. Integrar controles dentro del owner.
11. Implementar D3D11/HLSL mínimo y preview.
12. Migrar virtual camera a NV12/transport v2.
13. Añadir filtros, perfiles y calibración.
14. Ejecutar el laboratorio de IA.

## Progreso 2026-08-12

Completado del primer lote:

- workspace Rust creado;
- contratos IPC versionados y errores tipados;
- máquina de estados y lease exclusivo probados;
- identidad canónica entre DirectShow y Media Foundation;
- `camera-host.exe` nativo, persistente y reiniciable;
- enumeración Media Foundation por symbolic link;
- enumeración explícita de formatos por cámara;
- probe real de `IMFSourceReader` accesible desde la UI;
- build/packaging del host integrado en `prepare-assets.ps1` y Tauri;
- CI ampliada a todo el workspace;
- ADR inicial con resultados de UVC Camera y Pyle.

Siguiente lote:

1. mejorar métricas Source Reader separando startup y steady-state;
2. implementar MediaCapture/MediaFrameReader;
3. implementar IMFCaptureEngine;
4. introducir named pipe, heartbeat, deadlines y job object;
5. comparar backends y escribir la ADR de selección definitiva.

## Lo que no se hará en el primer lote

- borrar inmediatamente el backend funcional;
- añadir filtros nuevos sobre la arquitectura de ownership actual;
- descargar modelos de IA por intuición;
- introducir GStreamer, OpenCV, ONNX, OCIO y libplacebo juntos;
- prometer zero-copy antes de medir cada frontera;
- hacer reset PnP automático;
- migrar la UI fuera de Tauri.
