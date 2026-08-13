# Arquitectura de Webcam-Control

> Estado comprobado en Windows 11 con la cámara física `UVC Camera`. Tauri es la frontera de UI; la captura y el procesamiento viven fuera del WebView.

## Componentes y responsabilidades

| Componente | Responsabilidad |
|---|---|
| `ui/` | Presentación, perfiles y acciones; recibe solo el JPEG más reciente para preview. |
| `src-tauri/` | API de UI, leases exclusivos, ciclo de vida, validación, logs y bandeja. |
| `camera-protocol` | Contratos IPC versionados, límites y errores tipados. |
| `camera-domain` | Estado y ownership de cámaras, independiente de APIs Windows. |
| `windows-camera` | Inventario y captura WinRT/MF; normaliza identidades entre backends. |
| `camera-host` | Proceso persistente propietario de `MediaCapture` y del pipeline. |
| `camera-processing` | Grafo CPU determinista y ordenado: nodos repetibles de color, lente, LUT, plugins declarativos y escaladores. |
| `camera-frame` | Transporte mmap `CTFRAME2`, triple búfer latest-frame, publicación atómica y heartbeat. |
| `bridge/` | Acceso transitorio a controles DirectShow del driver; no transporta vídeo. |
| Media Source | Consumidor persistente de `CTFRAME2` expuesto por Frame Server como cámara virtual. |

## Flujo de vídeo

```text
webcam física
  → MediaCapture + MediaFrameReader (BGRA)
  → corrección de lente
  → FilterGraph ordenado (0–64 nodos, repetibles, bypass individual)
  → bilinear / Lanczos3
  → libyuv BGRA→NV12 (sólo salida virtual)
  → frame-v3.bin (CTFRAME2, tres slots NV12 de capacidad 4K fija; BGRA para preview)
  → Media Foundation Frame Server
  → Discord / Zoom / navegador
```

Preview y salida virtual usan el mismo host y pipeline, pero nunca simultáneamente sobre el mismo dispositivo. Cambiar de modo cierra el productor anterior, espera el worker y solo entonces transfiere el lease. Los errores de aplicación no reinician el host; los errores de transporte permiten un único reinicio controlado.

El escape de compatibilidad del driver consulta `ISpecifyPropertyPages` y abre su diálogo modal con `OleCreatePropertyFrame`, siguiendo el [patrón documentado por Microsoft](https://learn.microsoft.com/windows/win32/directshow/displaying-a-filters-property-pages). La aplicación pausa primero la captura, conserva el lease hasta cerrar el modal y vuelve a leer los controles; no intenta interpretar payloads propietarios desconocidos.

## Integridad y recuperación

- `Open` no responde hasta que existe un primer frame completo.
- El escritor llena un slot distinto del publicado y publica un único token atómico al terminar.
- El lector valida slot, secuencia y token antes y después de copiar; nunca espera al productor.
- La Media Source v10 mantiene abierto el mapping 4K de tamaño fijo, copia NV12 directamente cuando coincide la resolución y regula la entrega al FPS negociado.
- El lector rechaza cabeceras, tamaños, formatos, generaciones, frames parciales e inactividad inválidos.
- La preview considera bloqueada una fuente que no actualiza durante tres segundos.
- `Close`, salida de la aplicación y `Drop` señalizan al worker, esperan su terminación y marcan el frame inactivo.
- Controles del driver y captura pasan por el coordinador para evitar dueños concurrentes.

## Procesamiento

El backend CPU es la referencia correcta y el fallback compatible. Cada filtro vive en su propio módulo, el orden visible es el orden ejecutado y un grafo vacío es bypass real. Valida dimensiones, IDs, cantidades y rangos; conserva alfa; usa muestreo bilinear para distorsión Brown–Conrady, interpolación trilineal para LUT `.cube`, matrices 3×4 para plugins declarativos, `ARGBScale` SIMD para bilinear rápido y un Lanczos3 separable para calidad. El futuro backend D3D11 deberá compararse contra esta implementación antes de convertirse en fast path.

El modo IA es un valor explícito del protocolo, pero hoy devuelve un error claro y permanece deshabilitado en UI. Solo se habilitará con modelo ONNX redistribuible, hash fijado, pruebas de calidad y presupuesto de latencia; siempre conservará Lanczos como fallback.

## Cámara virtual

La DLL deriva del ejemplo MIT Windows-Camera y se registra mediante `MFCreateVirtualCamera`. Es un componente Media Foundation de usuario, no un driver de kernel. La revisión v10 acepta CTFRAME2 BGRA/NV12, ofrece 640×360 a 3840×2160 y 15–60 FPS, y prioriza NV12. Instalar o quitar requiere elevación por el registro bajo HKLM.
