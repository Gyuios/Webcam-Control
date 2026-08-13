# ADR 0001 — Host nativo y primer spike Media Foundation

Estado: aceptado; MediaCapture seleccionado para el camino normal
Fecha: 2026-08-12

## Contexto

La implementación funcional inicial abre las cámaras mediante FFmpeg/DirectShow
y usa otro proceso C# para controles DirectShow. La investigación recomienda un
único propietario reiniciable por cámara y Media Foundation como ruta primaria.

## Decisión

- Se crea un workspace Rust con `camera-protocol`, `camera-domain`,
  `windows-camera` y `camera-host`.
- Tauri inicia y supervisa el host como proceso externo.
- El contrato IPC tiene versión, request ID, errores tipados y límite de tamaño.
- Los leases usan una identidad canónica independiente del GUID de interfaz de
  DirectShow o Media Foundation.
- El backend actual continúa siendo el camino de producción mientras se miden
  las opciones nativas.
- El primer spike implementado es `IMFSourceReader`: inventario de dispositivos,
  enumeración explícita de media types y captura cronometrada de muestras.

## Evidencia inicial

Hardware probado:

| Dispositivo | Media types observados |
|---|---|
| UVC Camera, VID 12D1 / PID 4321 | 10 modos; NV12, YUY2, MJPEG y H.264; hasta 1920×1080@30 |
| Pyle LiveGamer PLINK4, VID 32ED / PID 3200 | 10 modos; NV12, YUY2 y RGB32; hasta 3840×2160@30 y 2560×1440@60 según formato |

Prueba UVC Camera, Source Reader, NV12 1920×1080@30, 30 muestras:

- primer frame observado por la UI: 271 ms;
- 30 muestras recibidas;
- duración de la lectura bloqueante: aproximadamente 1,67 s en esa ejecución;
- la preview FFmpeg pudo abrirse inmediatamente después sin desconectar la
  cámara, demostrando teardown correcto en este ciclo.

El FPS derivado de duración total del probe incluye startup y por eso no debe
interpretarse como cadence estable. La siguiente versión de métricas separará
startup, intervalos entre timestamps y throughput steady-state.

## Consecuencias

- El proyecto ya tiene una frontera donde incorporar MediaCapture,
  IMFCaptureEngine, D3D11 y controles sin inflar el proceso Tauri.
- El host todavía usa stdout JSON Lines. Named pipes, deadlines reales,
  cancelación, heartbeat y job objects siguen pendientes.
- Source Reader queda validado como candidato, no seleccionado como ganador.
- Enumerar formatos activa brevemente el dispositivo; la UI lo hace sólo con un
  lease de diagnóstico y nunca durante preview/salida.
- Los perfiles existentes se migran de forma natural porque la UI ahora recibe
  el ID canónico, aunque se debe añadir una migración explícita desde IDs v3
  antiguos antes de una release pública.

## Próxima decisión

Implementar y medir MediaCapture/MediaFrameReader e IMFCaptureEngine con el mismo
contrato. No promover Source Reader a backend normal hasta comparar:

- superficies D3D11;
- busy/privacy/hotplug;
- controles durante streaming;
- timestamps y cadence steady-state;
- recuperación tras cierre forzado y sleep/resume.

## Actualización de implementación

El spike `MediaCapture`/`MediaFrameReader` completó la misma matriz en la UVC y se promovió a backend normal. El host ahora conserva una sesión persistente, copia BGRA validado, procesa filtros/LUT/lente, reescala y publica mediante `CTFRAME2` triple-slot. Preview y productor virtual ya no usan FFmpeg. Una prueba física adicional capturó 1280×720 y produjo 640×360 Lanczos3 con cambio de filtros y LUT en caliente; `Close` liberó el dispositivo.
