# CameraTuner / Webcam-Control — contexto técnico integral

Última actualización: 2026-08-13
Estado documentado: rama de trabajo local posterior a la integración de captura nativa, filtros, cámara virtual y optimización inicial de preview.

Este documento es el punto de entrada principal para una persona o una inteligencia artificial que vaya a trabajar en el repositorio. Debe leerse antes de modificar la arquitectura. Explica qué producto se está construyendo, qué funciona hoy, por qué existe cada lenguaje, cómo circula un cuadro de video, qué partes todavía son provisionales y cómo validar cambios sin romper una webcam o dejar procesos huérfanos.

No sustituye el código ni las decisiones detalladas de `docs/`, pero consolida el contexto que normalmente se perdería entre conversaciones.

Para trabajo sobre rendimiento, transporte de cuadros, GPU, cámara virtual o reescalado IA, también debe leerse [`research/REFERENCE-ARCHITECTURE.md`](research/REFERENCE-ARCHITECTURE.md). Ese documento compara implementaciones externas en revisiones fijadas y define la migración propuesta a triple búfer NV12, D3D11 y DirectML.

---

## 1. Identidad y objetivo del proyecto

El repositorio original se llama **Webcam-Control**. El nombre de producto que aparece actualmente en la interfaz y en los binarios es **CameraTuner**. El proyecto es una colaboración entre sus contribuidores; no pertenece a una IA ni debe describirse como “el programa de la IA”.

El objetivo es ofrecer una aplicación de Windows mucho más pequeña y específica que OBS para:

1. enumerar webcams y capturadoras;
2. modificar los controles físicos que el driver realmente expone;
3. abrir el panel original del fabricante cuando existan controles propietarios;
4. capturar video con baja latencia;
5. aplicar filtros digitales, LUT, corrección óptica y reescalado;
6. publicar el resultado como una cámara virtual consumible por Discord, Zoom, navegadores y otras aplicaciones;
7. guardar perfiles por dispositivo;
8. diagnosticar fallos locales sin telemetría obligatoria.

El único requisito tecnológico permanente definido por los autores es conservar **Tauri para la interfaz**. El resto de la arquitectura puede cambiar si una alternativa demuestra mejor estabilidad, compatibilidad o rendimiento.

### Principios del producto

- Una cámara física tiene un único propietario activo dentro de CameraTuner.
- Ninguna API ni control inexistente se simula como si lo expusiera el hardware.
- Los controles del driver y los filtros digitales son conceptos diferentes.
- No se instala un driver de kernel para la cámara virtual.
- Las colas de video son acotadas y se prioriza siempre el cuadro más reciente.
- Un fallo del backend nativo no debe derribar la UI ni bloquear para siempre la webcam.
- “IA” sólo se muestra cuando existe un modelo real; Lanczos o bilinear no se venden como IA.
- La optimización se decide con mediciones, sin integrar frameworks enormes por intuición.

---

## 2. Estado funcional actual

### Implementado

- UI Tauri 2 con HTML, CSS y JavaScript sin framework web pesado.
- Enumeración de dispositivos DirectShow y Media Foundation.
- Identidad canónica para relacionar el mismo dispositivo visto por APIs diferentes.
- Controles `IAMCameraControl` y `IAMVideoProcAmp` expuestos por cada driver.
- Lectura de rango, paso, valor, valor predeterminado y modos automático/manual.
- Escritura validada de controles físicos.
- Apertura serializada de la property page original del driver.
- Captura persistente mediante `MediaCapture` + `MediaFrameReader`.
- Selector persistente por cámara de formato de entrada nativo; valida resolución, FPS y formato contra el inventario real antes de abrir la sesión.
- Preview JPEG en la UI con watchdog de cuadros.
- Procesamiento BGRA de referencia en CPU.
- Brillo digital, contraste, saturación, temperatura, matiz, gamma y flips.
- Corrección Brown–Conrady.
- LUT 3D `.cube` con interpolación y fuerza regulable.
- Grafo ordenable mediante flechas de hasta 64 filtros, con instancias repetibles y bypass individual.
- Plugins declarativos v1 con parámetros dinámicos y matriz de color segura.
- Instalación persistente de plugins desde el catálogo de **Agregar filtro**.
- Campos numéricos editables junto a todos los sliders de filtros.
- Reescalado bilinear y Lanczos3.
- Transporte versionado BGRA/NV12 mediante archivo mapeado en memoria.
- Cámara virtual de usuario basada en Media Foundation de Windows 11.
- Instalación desde la propia UI con UAC sólo para registrar una vez la Media Source y preparar el almacenamiento compartido.
- Perfiles locales por cámara con migración desde el formato v3.
- Logs JSON Lines estructurados, rotación, límites de tamaño y panic hook.
- Centro de notificaciones persistente con hora, origen, código e historial.
- Pruebas Rust y JavaScript; Clippy y compilación nativa con warnings tratados como errores.
- Empaquetado NSIS x64.
- Iconografía SVG Tabler vendorizada como sprite inline, sin fuentes ni dependencia de red en ejecución.

### Parcial o provisional

- Los filtros de producción todavía usan CPU y BGRA internamente; la salida virtual convierte una sola vez a NV12 con libyuv SIMD. D3D11/HLSL es la dirección futura.
- El preview cruza el IPC de Tauri como JPEG. Es una ruta simple y compatible, no zero-copy.
- La Media Source virtual v10 mantiene mapeado `CTFRAME2`, consume NV12 directamente, adapta el frame a la resolución negociada preservando el aspect ratio y regula la entrega al FPS negociado. BGRA/RGB32 queda como fallback de compatibilidad.
- El bridge C# sigue siendo necesario para controles DirectShow y property pages.
- La salida virtual hereda la captura y admite 640×360, 640×480, 1280×720, 1920×1080, 2560×1440 y 3840×2160; la Media Source anuncia frecuencias habituales entre 15 y 60 FPS.
- El estado de cámara virtual se comprueba mediante registro, almacenamiento y marcador local. La ruta completa fue validada con Discord y mediante un consumidor MediaCapture automatizado el 13 de agosto de 2026: UVC → BGRA procesado → libyuv → `CTFRAME2` NV12 → Media Source v10 → consumidor, sin cuadros azules o negros.
- El instalador es per-machine. La cámara se crea con acceso `CurrentUser`, pero el registro COM de la DLL sigue siendo HKLM.

### No implementado todavía

- Reescalado por IA / superresolución ONNX.
- Backend D3D11/HLSL de procesamiento.
- Texturas compartidas entre procesos.
- Preview nativa por DirectComposition dentro del HWND de Tauri.
- Curvas RGB/luma, sharpen, blur, denoise o chroma key.
- Calibración guiada ChArUco/fisheye con OpenCV.
- Hotplug y recuperación exhaustiva tras sleep/resume.
- Varios hosts simultáneos para varias cámaras activas.
- ARM64.
- Firma Authenticode de distribución.

---

## 3. Tecnologías y por qué existen

| Tecnología | Responsabilidad actual | Razón |
|---|---|---|
| Tauri 2 | Ventana, comandos, bandeja y empaquetado | UI liviana y requisito del proyecto. |
| HTML/CSS/JavaScript | Interfaz y perfiles locales | Evita un framework innecesario para la UI actual. |
| Rust | Orquestación, contratos, captura, frames, filtros y seguridad de estado | Buen control de memoria/concurrencia y binarios nativos pequeños. |
| `windows-rs` | Media Foundation, MediaCapture y APIs Windows | Binding oficial y tipado desde Rust. |
| C#/.NET | Bridge DirectShow | COM interop práctico para IAM y property pages existentes. |
| C++ | Media Source virtual y helper de instalación | La muestra/API de cámara virtual de Microsoft está en este ecosistema y necesita COM nativo. |
| PowerShell | Setup, doctor y build reproducible de assets | Automatización adecuada para toolchains Windows. |
| NSIS | Instalador Windows | Integración soportada por Tauri y hooks de registro/desinstalación. |

No hay un motivo arquitectónico para reemplazar Tauri. Tampoco hay un motivo para forzar todo el proyecto a un único lenguaje: cada frontera nativa tiene necesidades diferentes. El objetivo es reducir fronteras costosas por cuadro, no reducir la lista de lenguajes por estética.

FFmpeg fue retirado de la ruta normal y del bundle actual. Puede seguir siendo una herramienta de diagnóstico o comparación histórica, pero no debe abrir simultáneamente la misma cámara que el host nativo.

---

## 4. Mapa del repositorio

```text
CameraTuner/
├─ ui/
│  ├─ index.html                 estructura visual
│  ├─ styles.css                 diseño y estados
│  ├─ app.js                     controlador de UI y comandos Tauri
│  └─ lib.js                     funciones puras y migración de perfiles
├─ src-tauri/
│  ├─ src/main.rs                composición de la app y comandos Tauri
│  ├─ src/native_host.rs         supervisor/cliente de camera-host
│  ├─ src/coordinator.rs         leases exclusivos de dispositivos
│  ├─ src/diagnostics.rs         logs estructurados y rotación
│  ├─ src/models.rs              DTO de frontera con JavaScript
│  ├─ src/assets.rs              resolución segura de sidecars/assets
│  ├─ binaries/                  assets nativos generados, no fuente primaria
│  ├─ windows/installer-hooks.nsh registro/desregistro en instalación
│  └─ tauri.conf.json            ventana, CSP, bundle y recursos
├─ crates/
│  ├─ camera-protocol/           IPC versionado y tipos compartidos
│  ├─ camera-domain/             ownership, leases y máquina de estados
│  ├─ windows-camera/            APIs de captura Windows
│  ├─ camera-frame/              transporte CTFRAME2 triple-slot por mmap
│  ├─ camera-processing/         filtros/scalers CPU de referencia
│  └─ camera-host/               proceso persistente dueño de la captura
├─ bridge/
│  ├─ Program.cs                 controles y property pages DirectShow
│  └─ ControlWebcamBridge.csproj build .NET x64 autocontenido
├─ native/
│  ├─ virtual-camera-control/    helper C++ de ciclo de vida/UAC
│  └─ windows-camera.patch       parche reproducible a muestra de Microsoft
├─ scripts/
│  ├─ doctor.ps1                 inspección de toolchain
│  ├─ setup-toolchain.ps1        preparación de dependencias
│  └─ prepare-assets.ps1         build/verificación de sidecars nativos
├─ tests/                        pruebas JavaScript
├─ docs/                         arquitectura, investigación, ADR y plan
├─ Cargo.toml                    workspace Rust
├─ package.json                  scripts Node/Tauri
├─ README.md                     introducción corta
├─ THIRD_PARTY_NOTICES.md        atribuciones
└─ PROJECT-CONTEXT.md            este documento
```

Los ejecutables y DLL de `src-tauri/binaries` se regeneran. El código fuente está en `crates/`, `bridge/` y `native/`. No se debe corregir un binario manualmente.

---

## 5. Arquitectura en ejecución

```text
┌───────────────────────────────────────────────────────────────────┐
│ Tauri/WebView                                                     │
│ UI, selección, sliders, perfiles, estado y preview <img>          │
└────────────────────────────┬──────────────────────────────────────┘
                             │ comandos Tauri / bytes JPEG
┌────────────────────────────▼──────────────────────────────────────┐
│ Proceso camera-tuner.exe (Rust)                                   │
│ coordinación, leases, watchdogs, logs, bridge y camera-host IPC   │
└───────────────┬───────────────────────────────┬───────────────────┘
                │ JSON Lines                    │ JSON Lines
┌───────────────▼──────────────┐  ┌────────────▼────────────────────┐
│ Bridge DirectShow C#         │  │ camera-host.exe Rust            │
│ IAM controls/property page   │  │ dueño persistente de captura    │
└──────────────────────────────┘  └────────────┬────────────────────┘
                                               │ MediaFrameReader
                                   ┌───────────▼────────────────────┐
                                   │ cámara física                  │
                                   └───────────┬────────────────────┘
                                               │ BGRA latest-frame
                                   ┌───────────▼────────────────────┐
                                   │ filtros + lente + LUT + escala │
                                   └───────────┬────────────────────┘
                                               │ mmap CTFRAME2
                   ┌───────────────────────────▼────────────────────┐
                   │ C:\ProgramData\CameraTuner\frame-v3.bin       │
                   └───────────────┬───────────────────┬────────────┘
                                   │                   │
                         preview reader        Media Source C++
                         + JPEG Tauri          BGRA→NV12
                                                       │
                                           CameraTuner Virtual Camera
                                                       │
                                           Discord / Zoom / navegador
```

### Regla de ownership

`camera-domain` entrega un lease por ID canónico y propósito. Preview y salida virtual no pueden poseer a la vez la misma cámara. Esto evita el escenario clásico donde FFmpeg, DirectShow y Media Foundation compiten por un dispositivo UVC y lo dejan en un estado extraño.

El bridge DirectShow abre el filtro sólo durante una solicitud de control o durante la property page. Estas operaciones deben seguir coordinadas con la captura. Si se agrega una API nueva, debe pasar por el mismo coordinador; nunca debe abrir el dispositivo directamente desde JavaScript.

---

## 6. Inicio de la aplicación

### Lanzador local

En la máquina de desarrollo existe:

```text
C:\Users\Coty\Desktop\Iniciar CameraTuner.bat
```

El `.bat`:

1. define la raíz del repositorio;
2. comprueba `target\release\camera-tuner.exe`;
3. muestra una explicación si todavía no fue compilado;
4. cambia al directorio del proyecto;
5. inicia el ejecutable Release sin dejar una consola abierta.

Si el repositorio cambia de carpeta, hay que actualizar las dos rutas del `.bat`.

### Desarrollo

```powershell
npm ci
npm run doctor
npm run assets
npm run dev
```

`npm run dev:prepared` combina preparación de assets y Tauri dev.

### Release

```powershell
npm run build
```

Esto prepara los sidecars, compila Rust Release y genera el instalador NSIS. El lanzador del Escritorio usa el ejecutable Release del workspace, no instala automáticamente el MSI/NSIS.

---

## 7. Descubrimiento de dispositivos y controles físicos

### IDs

DirectShow y Media Foundation pueden devolver symbolic links distintos para el mismo USB. `canonical_device_id`:

1. elimina espacios externos;
2. convierte a minúsculas;
3. descarta el sufijo iniciado en `#{...}`;
4. conserva el prefijo físico estable para coordinar y guardar perfiles.

El backend conserva internamente su ID completo. Los symbolic links no deben registrarse completos en logs.

### Controles que puede mostrar la UI

`IAMCameraControl`:

- exposición;
- zoom;
- iris;
- enfoque.

`IAMVideoProcAmp`:

- brillo;
- contraste;
- tono;
- saturación;
- nitidez;
- gamma;
- balance de blancos;
- compensación de contraluz;
- ganancia.

La lista no garantiza que todos aparezcan. El bridge llama `GetRange` y `Get`; si el driver rechaza una propiedad, esa propiedad se omite. Por eso una UVC Camera puede ofrecer sólo exposición y brillo mientras otra webcam ofrece muchos más.

Cada control transporta mínimo, máximo, step, default, valor actual, soporte auto/manual y modo actual. Antes de escribir se valida:

- que la propiedad pertenezca al enum permitido;
- que el modo exista;
- que el valor esté en rango;
- que respete el incremento del driver.

No se deben reemplazar estos rangos por sliders fijos universales.

### Panel original

El bridge consulta `ISpecifyPropertyPages` y abre `OleCreatePropertyFrame`. Este panel puede exponer Extension Units o parámetros propietarios que no es seguro normalizar. Debe mantenerse como fallback de compatibilidad incluso si la UI incorpora más controles propios.

---

## 8. Captura nativa

El backend normal es `MediaCapture` + `MediaFrameReader` en el crate `windows-camera`. El selector también enumera `MediaFrameSource.SupportedFormats`; no mezcla el inventario de SourceReader con una captura MediaFrameReader. Esto evita ofrecer H.264 anunciado por Media Foundation pero imposible de seleccionar en la ruta WinRT real.

Flujo simplificado:

1. Tauri enumera formatos de la cámara seleccionada.
2. Elige un formato explícito.
3. Adquiere el lease.
4. Inicia o reutiliza `camera-host.exe`.
5. Envía `HostCommand::Open` por JSON Lines.
6. El host crea un hilo de captura.
7. `MediaFrameReader` conserva como máximo el último frame disponible.
8. Se convierte a BGRA cuando es necesario.
9. Se aplican filtros y escala.
10. `FrameWriter` publica el frame comprometido.
11. El host confirma `Open` sólo después del primer frame.

El timeout inicial actual es 12 segundos. Si falla, se activa el stop flag, se une el worker y se devuelve un error tipado.

### Elección de formato para preview

La UI enumera cada combinación nativa de resolución, FPS y formato de píxel. La preferencia se conserva por cámara y el backend exige que coincida exactamente con un modo anunciado en ese momento. Si el usuario mantiene **Automático**, la ruta interactiva prioriza:

1. 640×360;
2. 640×480;
3. 1280×720;
4. 1920×1080;
5. otros formatos válidos.

Dentro de una resolución favorece aproximadamente 24–30 fps y formatos NV12/YUY2 antes que MJPEG/H.264/BGRA.

La salida virtual usa el modo explícito elegido por el usuario y hereda sus dimensiones. En **Automático**, selecciona un modo nativo seguro; si la webcam usa dimensiones no anunciadas por la Media Source virtual, se elige la resolución compatible de aspect ratio y área más cercanos. El reescalado queda desactivado cuando entrada y salida coinciden.

---

## 9. Pipeline de procesamiento

El orden ya no está fijado en código. El motor recorre exactamente el arreglo `FilterGraph.nodes` que muestra la UI:

```text
BGRA de entrada
  → nodo 1 elegido por el usuario
  → nodo 2 (puede repetir el mismo tipo)
  → …
  → nodo N
  → resize de salida sólo si cambian dimensiones
  → BGRA/preview o libyuv NV12/salida virtual
  → CTFRAME2
```

### Fast paths obligatorios

El pipeline debe ser barato cuando el usuario no aplica efectos. Desde la optimización del 2026-08-13:

- no revalida settings iguales en cada frame;
- no recorre píxeles con ajustes de color neutros;
- no ejecuta `powf` cuando gamma es 1;
- no calcula luminancia cuando saturación es 1;
- no llama LUT con fuerza 0;
- no ejecuta flips cuando ambos están desactivados;
- no hace resize si las dimensiones ya coinciden;
- sólo clona y valida el grafo cuando cambia su revisión, no en cada frame;
- reutiliza el `Buffer` WinRT y el `Vec<u8>` que reciben BGRA durante toda la sesión;
- usa `ARGBScale` SIMD de libyuv para `FastBilinear`, conservando Lanczos3 como ruta de calidad.

Antes de este cambio, un frame 1280×720 neutro ejecutaba aproximadamente 2,76 millones de potencias flotantes por cuadro: tres canales por 921.600 píxeles. Era una causa directa de CPU alta y preview pesada.

### Ajustes digitales

Cada instancia vive en un `FilterNode` con ID estable, estado enabled, etiqueta opcional y un `FilterEffect` tipado. `FilterGraph` admite como máximo 64 nodos. Los rangos normalizados son:

- brillo `[-0.5, 0.5]`;
- contraste `[0, 2]`;
- saturación `[0, 2]`;
- temperatura `[-0.5, 0.5]`;
- tint `[-0.5, 0.5]`;
- gamma `[0.25, 2.5]`;
- fuerza LUT `[0, 1]`.

Son filtros posteriores a la captura. No modifican registros de la cámara y no reemplazan exposición, ganancia o white balance físicos.

### Corrección de lente

Se usa el modelo Brown–Conrady con `k1`, `k2`, `k3`, `p1`, `p2` y scale. Los rangos defensivos son K1 `[-0.5, 0.5]`, K2 `[-0.25, 0.25]`, K3 `[-0.1, 0.1]`, P1/P2 `[-0.05, 0.05]` y escala `[-0.25, 0.5]`. Los sliders usan pasos finos y todos los valores se pueden escribir a mano. La implementación CPU hace remuestreo bilinear y usa un buffer scratch reutilizable.

El modo actual es útil como referencia, pero una corrección activa a 720p/30 en CPU puede consumir mucho. El destino de producción es precalcular un mapa UV y aplicarlo en shader.

### LUT `.cube`

El parser:

- acepta LUT 3D de tamaño 2 a 65;
- valida cantidad exacta de entradas;
- valida dominio finito y creciente;
- rechaza LUT 1D;
- interpola tridimensionalmente;
- permite blend por fuerza.

Cada nodo LUT referencia un `assetId`; el contenido se valida, se conserva fuera de los perfiles en la biblioteca de datos de la aplicación y se vuelve a cargar al iniciar. Esto permite varias LUT en distintas posiciones sin duplicar archivos de varios MiB dentro de localStorage.

### Plugins de filtros

La ABI v1 se encuentra en `filter-plugin-sdk/`. Un plugin es un manifiesto JSON de datos, no una DLL ejecutable. Puede declarar hasta 32 sliders y modular una matriz RGB 3×4. CameraTuner limita tamaño, identificadores, rangos y coeficientes antes de enviarlo al host. **Agregar filtro** separa filtros incluidos y plugins instalados; **Instalar plugin** valida un JSON de hasta 256 KiB, lo guarda bajo el ID canónico en los datos de la app y lo conserva para futuras sesiones.

Esta frontera permite filtros custom de color sin arriesgar el proceso de captura. Shaders o WebAssembly aislado son extensiones futuras; nunca se debe cargar código nativo arbitrario en `camera-host`.

Los límites son importantes porque el archivo es entrada no confiable.

### Escalers

- `FastBilinear`: menor coste, apropiado para preview/fallback.
- `QualityLanczos3`: más calidad y más CPU.
- `Ai`: enum reservado; devuelve error explícito mientras no exista backend ONNX.

---

## 10. Transporte CTFRAME2

Archivo normal:

```text
C:\ProgramData\CameraTuner\frame-v3.bin
```

Es un archivo mapeado, no un video grabado. Contiene un encabezado global de 64 bytes y tres slots alineados. Cada slot posee su propio encabezado de 64 bytes y capacidad para un payload completo. La preview publica BGRA; la salida virtual publica NV12 compacto.

### Encabezado global

| Offset | Tipo | Significado |
|---:|---|---|
| 0 | 8 bytes | magic ASCII `CTFRAME2` |
| 8 | u32 LE | tamaño de encabezado, 64 |
| 12 | u32 LE | cantidad de slots, 3 |
| 16 | u32 LE | separación alineada entre slots |
| 20 | u32 LE | capacidad máxima del payload |
| 24 | u32 LE | pixel format esperado, 1 = BGRA, 2 = NV12 |
| 28 | u32 atómico | active, 0/1 |
| 32 | u64 atómico | token de publicación: sequence + slot |
| 40 | u64 LE | generación de la sesión |
| 48 | u64 atómico | heartbeat en microsegundos Unix |
| 56–63 | reservado | cero actualmente |

### Encabezado de cada slot

Incluye sequence, timestamp, width, height, stride, formato, tamaño, estado y generación. El payload comienza 64 bytes después del inicio del slot.

### Publicación latest-frame

El writer:

1. elige el siguiente de los tres slots, siempre distinto del publicado;
2. lo marca como `writing`;
3. escribe metadata y payload;
4. lo marca como `published` con semántica release;
5. publica atómicamente un token que combina sequence y slot.

El reader:

1. carga el token con semántica acquire;
2. evita copiar si ya consumió esa sequence;
3. valida estado, sequence, generación y límites;
4. copia el payload;
5. vuelve a leer estado, sequence y token;
6. acepta sólo si los tres continúan idénticos; de lo contrario reintenta contra el cuadro más reciente.

Así siempre existe al menos un cuadro publicado mientras el productor llena otro. No hay mutex ni espera entre procesos y un lector lento no genera una cola histórica.

Límites actuales:

- máximo 256 MiB por frame;
- BGRA de 4 bytes por píxel o NV12 de 1,5 bytes por píxel con dimensiones pares;
- tres slots por archivo;
- mapping persistente durante la sesión;
- heartbeat y generación para detectar productores detenidos o reiniciados;
- un archivo compartido global;
- una salida activa.

La salida virtual ya publica NV12 mediante la versión estable y fijada de `shiguredo_libyuv`/Google libyuv. Quedan pendientes seguridad por usuario explícita y varias instancias. La ruta rápida futura utilizará texturas D3D11 compartidas conservando CTFRAME2 como fallback CPU.

---

## 11. Preview y latencia

La ruta actual es:

```text
CTFRAME2 → FrameReader Rust → BGRA/RGB → JPEG → Response Tauri
→ Uint8Array → Blob URL → <img>
```

No usa Base64. El polling actual espera 25 ms entre respuestas; un frame repetido se detecta leyendo sólo el header y no copia varios megabytes.

El modo explícito se captura y procesa a su resolución real. Después de aplicar filtros, `camera-host` limita sólo la copia destinada al monitor a 960 píxeles de ancho, conservando el aspect ratio. El JPEG usa quality 74 y mantiene una segunda reducción defensiva para transportes antiguos o frames inesperados.

### Qué medir si vuelve a sentirse lenta

- formato real elegido y fps del driver;
- tiempo hasta primer frame;
- CPU de `camera-host.exe`;
- CPU de `camera-tuner.exe` durante JPEG;
- secuencias, heartbeat y timestamps de CTFRAME2;
- edad del frame al mostrarse;
- existencia de lens correction, LUT o Lanczos activos;
- si otra aplicación abre la webcam;
- throttling USB, hub o formato MJPEG/H.264 problemático.

No se debe “arreglar” la latencia agregando una cola. En webcam, una cola conserva cuadros viejos y empeora la demora. Se descartan frames y se procesa el más reciente.

### Evolución recomendada

1. añadir métricas agregadas por sesión: fps, frame age y p50/p95 de procesamiento;
2. migrar color/LUT/lente/scale a D3D11;
3. comparar preview nativa DirectComposition con JPEG fallback;
4. evitar copia BGRA→RGB cuando una API de encoder acepte formato nativo;
5. preservar siempre una ruta CPU compatible.

---

## 12. Cámara virtual

### Qué es

Es una cámara de software de **Media Foundation en user mode**, creada con:

- tipo `MFVirtualCameraType_SoftwareCameraSource`;
- lifetime `System`;
- acceso `CurrentUser`;
- nombre `CameraTuner Virtual Camera`;
- CLSID `{3429FF3A-676F-4B65-86D0-E70DFA72C54B}`.

No es un driver de kernel, por lo que no necesita firma de driver. Para una distribución profesional sí conviene firmar ejecutables, DLL e instalador con Authenticode para reputación/SmartScreen.

### Componentes

- `camera-tuner-media-source.dll`: Custom Media Source consumida por Windows Frame Server.
- `camera-tuner-virtual-camera-...exe`: instala, consulta y elimina la instancia.
- `windows-camera.patch`: genera la DLL desde una revisión fijada de la muestra oficial de Microsoft.
- `installer-hooks.nsh`: hace el registro durante una instalación NSIS per-machine.

### Por qué antes el botón no funcionaba

La UI llamaba sólo `MFCreateVirtualCamera`, pero el registro HKLM de la Media Source y los permisos de `ProgramData` se configuraban exclusivamente en el instalador NSIS. Al ejecutar directamente `target\release\camera-tuner.exe`, esos prerrequisitos no existían. El botón mostraba una capacidad que el entorno portable no había preparado.

### Flujo corregido

Al pulsar **Instalar cámara virtual**:

1. Tauri localiza la DLL empaquetada.
2. El helper comprueba que la DLL exista.
3. Si CLSID y almacenamiento ya están preparados, no eleva.
4. Si faltan, relanza sólo la preparación con verbo Windows `runas`.
5. El usuario acepta UAC.
6. El proceso elevado copia la Media Source a `%ProgramFiles%\CameraTuner`, registra desde allí `InProcServer32`, `ThreadingModel=Both` y prepara `C:\ProgramData\CameraTuner`.
7. Se concede Modify al grupo built-in Users usando ACL de Windows.
8. El proceso original, no elevado, crea/inicia la cámara para el usuario actual.
9. Se guarda un marcador HKCU de instalación.

Estados del helper:

- `source-not-registered`;
- `storage-not-ready`;
- `source-invalid` (el registro existe, pero `CoGetClassObject` no puede activar la clase);
- `not-installed`;
- `installed`.

El estado no confía solamente en el marcador HKCU.

La DLL no debe registrarse desde el repositorio, `Downloads` ni ninguna ruta bajo el perfil del usuario. `Frame Server` y `Frame Server Monitor` la cargan bajo cuentas de servicio; una DLL registrada desde `C:\Users\...` provoca `0x80070005` al ejecutar `IMFVirtualCamera::Start`. Además sería incorrecto permitir que un usuario sin elevar modifique código que luego carga un servicio. El código ejecutable vive en `Program Files`; sólo el transporte de frames mutable vive en `ProgramData`. Las revisiones nativas usan nombres versionados (`camera-tuner-media-source-v10.dll`, etc.) y CLSID nuevos cuando cambia el ABI, porque Windows puede mantener el módulo anterior cargado mientras un consumidor conserva la cámara abierta.

La resolución elegida por un consumidor no tiene por qué coincidir con la de `CTFRAME2`. Discord puede negociar 1280×720 aunque CameraTuner capture 1920×1080. La Media Source valida el layout y el slot publicado, crea un canvas del tamaño solicitado, escala conservando la relación de aspecto y centra con barras negras. No debe volver al generador sintético azul por una mera diferencia de dimensiones.

La Media Source v10 mantiene `frame-v3.bin` abierto y mapeado durante la sesión. El archivo reserva desde el inicio tres slots NV12 con capacidad 4K, de modo que cambiar entre 640p, 1080p y 4K no redimensiona una sección que Frame Server tenga abierta. Lee BGRA o NV12, verifica estado, sequence y token después de copiar y usa un fast path de copia NV12 cuando la resolución coincide. Para otra resolución escala los planos Y/UV preservando aspect ratio; RGB32 queda como fallback. Si una lectura aislada falla, conserva el último cuadro válido durante tres segundos; después emite negro hasta que vuelva la señal. También detecta heartbeat o secuencia estancados. Registra eventos compactos en `C:\ProgramData\CameraTuner\media-source-v10.log`.

Prueba física del 2026-08-13, UVC 640×360/30 durante 10 segundos: 100 muestras observadas, 100 secuencias nuevas, cero cuadros azules y cero negros. La conversión libyuv promedió aproximadamente 1,0 ms por frame; CTFRAME2 pasó de 921.600 bytes BGRA a 345.600 bytes NV12 por slot (62,5 % menos payload). Estos números son diagnóstico de esta máquina, no un benchmark universal.

La misma prueba con productor 1920×1080 y consumidor virtual 640×360 volvió a obtener 100/100 secuencias, cero azul y cero negro, validando el scaler NV12 de la Media Source. En 1080p la conversión BGRA→NV12 promedió ~9,0 ms/frame. La preview 1920×1080→960×540 bajó de ~9,7 a ~2,4 ms/frame al cambiar el bilinear Rust por `ARGBScale` SIMD. La deuda de rendimiento principal pasa a ser evitar BGRA/conversión a 1080p mediante D3D11 o una ruta NV12 nativa para grafos vacíos.

La v10 se validó además con dos sesiones consecutivas sobre el mismo Frame Server: 640×360 seguida por 1920×1080, ambas con consumidor 640×360. Cada sesión produjo 40/40 muestras distintas, cero azul y cero negro; `media-source-v10.log` registró cero headers inválidos, misses o fallbacks. `frame-v3.bin` conserva un tamaño fijo de 37.325.056 bytes entre ambas sesiones, evitando el error Windows 1224 de redimensionar una sección mapeada.

La muestra original de Microsoft entregaba una muestra inmediatamente por cada `RequestSample`, aunque anunciara 30 FPS. Un consumidor agresivo llegó a solicitar aproximadamente 740 cuadros por segundo, desperdiciando CPU en conversiones repetidas. La v10 calcula `m_frameDuration` desde `MF_MT_FRAME_RATE`, regula cada entrega con reloj Media Foundation y publica timestamps/duraciones monotónicos.

La UVC probada llegó a detener `MediaFrameReader` tras 7.808 frames. `camera-host` conserva ahora el último frame y reinicia automáticamente el stream cuando recibe el error recuperable `MediaFrameReader stalled`, con backoff entre 100 ms y 2 s. La UI consulta cada dos segundos el estado ligero del productor y deja de anunciar “salida activa” si el host realmente terminó.

### Activación de salida

Al activar output:

1. se valida que la cámara virtual esté instalada;
2. se hereda la resolución del formato físico y se valida contra las seis dimensiones soportadas;
3. se adquiere lease de la cámara física;
4. se abre capture con output igual a la entrada, salvo ajuste de compatibilidad para dimensiones inusuales;
5. comienza a actualizarse CTFRAME2;
6. la Media Source lee el último frame y expone NV12 al consumidor.

La cámara virtual puede aparecer instalada aunque la salida esté detenida. En ese estado el consumidor no recibe una fuente activa de CameraTuner. Una evolución futura debería mostrar un slate/fallback explícito.

### Desinstalación

El botón Quitar:

- detiene output;
- llama `IMFVirtualCamera::Remove`;
- borra el marcador del usuario.

El registro COM puede permanecer hasta desinstalar la aplicación. El hook NSIS elimina el CLSID y el archivo compartido durante uninstall. Esta separación evita pedir elevación para cada activación/desactivación.

### Prueba manual necesaria

La prueba real requiere interacción humana:

1. abrir el ejecutable Release;
2. pulsar Instalar;
3. aceptar UAC;
4. comprobar estado “Instalada y lista”;
5. seleccionar UVC Camera;
6. activar salida;
7. abrir la lista de cámaras de Discord/Zoom/Camera y elegir CameraTuner Virtual Camera;
8. confirmar imagen, orientación y filtros;
9. detener salida y comprobar liberación de la UVC;
10. reiniciar CameraTuner y validar persistencia.

Una IA no debe aceptar el UAC ni manejar las ventanas del usuario sin autorización explícita.

---

## 13. IPC y procesos

### camera-host

Usa JSON Lines en stdin/stdout. Cada request incluye:

- `protocolVersion`;
- `requestId`;
- deadline opcional;
- payload tipado.

La versión actual es **4**. Desde v4, `Open` exige `outputPixelFormat`: preview solicita `BGRA` y salida virtual solicita `NV12`. App y sidecar rechazan una mezcla de versiones antes de abrir la cámara; esto evita interpretar CTFRAME2 con un layout incorrecto tras una compilación parcial.

Cada response repite versión e ID y contiene `Result<HostResponse, HostError>`.

Errores estables:

- invalid request;
- protocol mismatch;
- deadline exceeded;
- device absent/busy/lost;
- privacy denied;
- unsupported format/control;
- driver rejected;
- backend unavailable;
- internal.

El stdout está reservado al protocolo. Los diagnósticos se escriben por stderr y Tauri los ingiere.

### Bridge DirectShow

También opera como servidor JSON Lines persistente con comandos:

- `list`;
- `controls <path>`;
- `set <path> <kind> <property> <value> <automatic>`;
- `property-page <path>`.

Tauri reinicia sidecars cuando una falla de transporte es recuperable. No se debe escribir texto arbitrario a stdout en ningún sidecar.

### Límites pendientes

El protocolo ya está versionado, pero aún se recomienda:

- named pipes con ACL por usuario;
- deadlines aplicados realmente en cada backend;
- heartbeat;
- Windows Job Object para terminar hijos huérfanos;
- process/session IDs en toda métrica;
- backoff explícito de reinicios.

---

## 14. Perfiles

Los perfiles se guardan hoy en localStorage bajo:

```text
camera-tuner-profiles-v5
```

Claves legacy:

```text
camera-tuner-profiles-v3
camera-tuner-profiles-v4
camera-tuner-profiles-v3
control-webcam-profiles-v3
```

Un perfil contiene controles físicos y el `FilterGraph` completo. La migración v3/v4 vive en `ui/lib.js` y tiene pruebas puras en `tests/ui-lib.test.js`. Los ajustes planos v4 se convierten sólo en nodos no neutros, conservando el resultado y evitando trabajo identidad.

Reglas:

- indexar por ID canónico, nunca sólo por friendly name;
- no copiar automáticamente un perfil de “Pyle” a “UVC Camera”;
- aplicar controles en secuencia y reportar fallos parciales;
- guardar filtros normalizados;
- versionar cualquier cambio de esquema;
- agregar pruebas de migración antes de subir la versión.

---

## 15. Diagnósticos

Archivo:

```text
<app_log_dir>\webcam-control.jsonl
```

La ruta concreta se obtiene desde Tauri y se abre con el botón de diagnósticos. Los archivos sobreviven cierres y reinicios de Windows; se limpian únicamente por la rotación de tamaño.

La campana superior guarda hasta 100 errores recientes en `camera-tuner-notifications-v1`. Cada entrada contiene hora, título, origen, mensaje y un código como `0x80040111` cuando puede extraerse. Es una vista de conveniencia: los JSONL siguen siendo la fuente completa de diagnóstico.

Cada registro incluye:

- timestamp Unix ms;
- uptime;
- session ID;
- PID;
- nombre de thread;
- level;
- subsystem;
- event;
- message;
- context.

Política actual:

- archivo activo máximo 5 MiB;
- cinco rotaciones retenidas;
- strings limitados a 16 Ki caracteres;
- objetos con profundidad máxima 8;
- arrays/objetos limitados a 100 elementos;
- panic hook;
- logs de stderr de sidecars integrados;
- IDs de cámaras redactados en eventos sensibles;
- no se guardan imágenes;
- no se envía telemetría remota.

“Registrar todo” no significa escribir un log por frame. A 30/60 fps eso degrada el pipeline, llena el disco y hace más difícil encontrar el error. Se registran transiciones, errores y métricas agregadas. Para datos por frame se usan contadores en memoria y un resumen periódico o al detener la sesión.

---

## 16. Seguridad y privilegios

Fronteras de confianza:

- WebView/UI;
- comandos Tauri;
- sidecars C#/Rust/C++;
- archivos `.cube`;
- CTFRAME2 en ProgramData;
- DLL Media Source cargada por Frame Server;
- futuros modelos ONNX;
- drivers de terceros.

Controles existentes:

- CSP Tauri;
- `freezePrototype`;
- comandos Tauri explícitos;
- validación de controles y filtros;
- límite de mensajes IPC de 16 MiB;
- límite de frames de 256 MiB;
- validación completa del header mmap;
- parser `.cube` limitado;
- IDs de LUT restringidos a un único componente seguro antes de tocar el filesystem;
- perfiles, preferencias y notificaciones persistidas normalizados antes de usarse;
- actualizaciones de filtros serializadas para conservar el orden elegido en la UI;
- paths de assets comprobados;
- dependencia Windows-Camera fijada por commit y SHA-256;
- UAC limitado a prerrequisitos machine-wide;
- ACL explícita para almacenamiento compartido;
- logs con truncado y redacción.

Pendientes importantes:

- threat model formal actualizado;
- validar firma/hash de sidecars en runtime;
- named pipe y ACL por usuario;
- evitar un archivo de frame global entre sesiones Windows;
- manifiestos/hashes/allowlist para modelos ONNX;
- fuzz del protocolo, CTFRAME2 y `.cube`;
- SBOM automatizado;
- firma Authenticode de releases.

Nunca se debe resetear un dispositivo PnP, editar drivers o elevar procesos automáticamente como estrategia de recuperación.

---

## 17. Rendimiento

### Presupuesto conceptual a 30 fps

Un frame dispone de 33,3 ms de extremo a extremo. No todo ese presupuesto pertenece a los filtros: captura, conversión, transporte, preview/consumer y scheduling también consumen tiempo.

Objetivos sugeridos:

- ninguna cola mayor a 1;
- p95 frame processing por debajo de 10 ms en la ruta GPU;
- frame age visible p95 por debajo de 100 ms;
- preview cerca del fps de fuente sin aumentar CPU cuando está oculto;
- idle sin captura: CPU aproximadamente cero;
- sin filtros: ruta de bypass real;
- memoria estable durante sesiones largas.

### Costes conocidos de la versión actual

- conversión de `SoftwareBitmap` a BGRA;
- copia de BGRA al `Vec<u8>` reutilizable de la sesión;
- filtros CPU cuando están activos;
- resize CPU si cambian dimensiones;
- copia a mmap;
- BGRA→RGB y JPEG para preview;
- lectura de archivo y BGRA→NV12 en la Media Source virtual.

### Qué no hacer

- no procesar 1080p para mostrar un preview pequeño si 360p está disponible;
- no ejecutar filtros identidad;
- no copiar frames repetidos;
- no usar Base64 por frame;
- no hacer flush a disco de todo el payload en cada frame;
- no escribir logs por frame;
- no acumular solicitudes de sliders;
- no ejecutar IA sin scheduler latest-frame y fallback.

### Próximo salto de rendimiento

El cambio decisivo será mantener NV12/texturas D3D11 el mayor tiempo posible:

```text
MediaCapture surface
→ shared D3D11 texture
→ HLSL graph
→ preview DirectComposition
→ virtual source NV12
```

CTFRAME2 debe permanecer como fallback de compatibilidad y oracle de pruebas cuando exista una ruta D3D11.

---

## 18. Plan de reescalado IA

La UI puede reservar la opción, pero el backend debe permanecer deshabilitado hasta integrar un modelo medido.

Arquitectura propuesta:

```text
latest frame
→ preprocessing GPU
→ inference slot único
→ validación temporal/deadline
→ composición
→ virtual camera
       ↘ si falla o vence: Lanczos/bilinear inmediato
```

Requisitos:

- interfaz `AiBackend` independiente de proveedor;
- comparar Windows ML y ONNX Runtime;
- providers para Intel/AMD/NVIDIA y CPU;
- máximo un frame pendiente;
- x1.5/x2 antes de considerar 4x;
- no bloquear controles ni cierre;
- fallback sin reiniciar cámara;
- modelo con nombre, versión, origen, licencia, SHA-256, opset, shapes y color space;
- benchmark de startup, p50/p95/p99, RAM, VRAM, consumo y frame age;
- dataset con caras, cabello, texto, movimiento, ruido y poca luz;
- evaluación temporal para flicker, no sólo capturas estáticas.

Un modelo no entra en stable si la nitidez subjetiva aumenta a costa de stutter o latencia notable.

---

## 19. Build reproducible y dependencias nativas

`scripts/prepare-assets.ps1`:

1. comprueba dotnet, cargo, git y MSBuild;
2. compila `camera-host` Release;
3. publica el bridge .NET autocontenido x64;
4. descarga un ZIP fijado de `microsoft/Windows-Camera`;
5. verifica SHA-256;
6. extrae en un directorio temporal validado;
7. aplica `native/windows-camera.patch`;
8. compila la Media Source C++;
9. compila el helper de cámara virtual;
10. copia artifacts sólo cuando cambiaron.

No actualizar commit, URL o hash por separado. Cualquier update de la muestra requiere:

- revisar el diff upstream;
- actualizar patch;
- actualizar hash;
- compilar limpio;
- repetir instalación/consumer tests;
- actualizar `THIRD_PARTY_NOTICES.md` si corresponde.

---

## 20. Verificación antes de entregar cambios

### Checks rápidos

```powershell
node --check ui/app.js
node --check ui/lib.js
node --test tests/*.test.js
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
dotnet build bridge/ControlWebcamBridge.csproj -c Release
dotnet format bridge/ControlWebcamBridge.csproj --verify-no-changes --no-restore
npm audit --audit-level=high
```

### Build completo

```powershell
npm run build
```

### Matriz manual mínima

- UVC Camera conectada directamente;
- capturadora Pyle identificada por separado;
- preview start/stop repetido;
- sliders físicos y auto/manual;
- filtros neutros y activos;
- property page abre/cierra;
- cámara ocupada por otra app;
- desconexión durante preview;
- reconexión y reapertura;
- instalación de cámara virtual con UAC;
- output visible en una aplicación consumidora;
- cierre de CameraTuner con output activo;
- relanzamiento y liberación de dispositivo.

### Soak futuro

- 24 horas de captura;
- 100 hotplug;
- 1.000 ciclos open/start/stop/reopen;
- sleep/resume;
- hub removal;
- 100 property pages;
- varios consumidores virtuales;
- upgrade/uninstall.

No se debe afirmar que una cámara virtual “funciona” sólo porque compila. Debe aparecer en el enumerador de un consumidor y entregar frames reales.

---

## 21. Problemas conocidos y recuperación segura

### Preview se detiene o webcam queda extraña

1. detener preview/output desde CameraTuner;
2. cerrar aplicaciones consumidoras;
3. comprobar procesos `camera-host`, navegador, Discord, Zoom, OBS, antivirus;
4. revisar logs;
5. esperar liberación del driver;
6. desconectar/reconectar físicamente si el firmware UVC quedó bloqueado.

No matar procesos ajenos ni resetear PnP sin autorización.

### Exposición bloqueada

Puede estar en modo automático. Cambiar a manual sólo si `supportsManual` es true. Algunos drivers publican escalas logarítmicas o valores negativos. La UI debe respetar min/max/step y leer de vuelta el valor real cuando se implemente readback posterior a set.

### Menos controles que otra webcam

Es normal: depende de lo que expone el driver. Usar property page para extensiones propietarias y filtros digitales para transformaciones posteriores.

### Antivirus

Sidecars nuevos, DLL COM y binarios sin firma pueden generar falsos positivos. No se deben desactivar protecciones como solución de producto. Para releases públicas: binarios reproducibles, hashes, firma Authenticode y reporte del falso positivo al proveedor.

---

## 22. Reglas para futuras modificaciones

1. Leer este documento, `docs/ARCHITECTURE.md` y el ADR relevante.
2. Inspeccionar cambios locales antes de editar; el worktree puede contener trabajo de ambos autores.
3. No borrar cambios ajenos para “limpiar” el repositorio.
4. Mantener Tauri como UI.
5. No abrir la webcam desde el WebView.
6. Pasar toda posesión por leases.
7. Mantener stdout de sidecars reservado al protocolo.
8. No introducir colas ilimitadas.
9. Agregar fast path neutral a cada filtro nuevo.
10. Validar toda entrada de archivo/IPC.
11. No llamar IA a un algoritmo clásico.
12. No agregar una dependencia grande sin benchmark y plan de packaging.
13. No registrar frames ni IDs sensibles completos.
14. Agregar prueba de regresión cuando se corrige un bug reproducible.
15. Ejecutar fmt, tests y Clippy antes de entregar.
16. Para C++, mantener `/W4 /WX`; para Rust, Clippy con warnings como error.
17. Actualizar este documento cuando cambie una frontera importante.

---

## 23. Decisiones que todavía pueden cambiar

No tratar estas decisiones actuales como dogma:

- bridge C# frente a controles absorbidos por el host;
- BGRA mmap frente a shared D3D11 texture;
- preview JPEG frente a DirectComposition;
- Windows ML frente a ONNX Runtime;
- NSIS per-machine frente a registro COM per-user viable;
- custom HLSL frente a una biblioteca puntual para operaciones complejas;
- un archivo global frente a canales por sesión.

Sí deben conservarse los objetivos: ownership único, latest-frame, validación, fallback, observabilidad y Tauri como UI.

---

## 24. Roadmap recomendado desde este estado

### Prioridad 0 — validar lo recién integrado

- prueba manual de preview optimizado con UVC Camera;
- medir CPU antes/después con filtros neutros;
- instalar cámara virtual desde UI y aceptar UAC;
- validar salida real en Discord/Zoom/Camera;
- capturar logs de cualquier fallo;
- comprobar cleanup tras cerrar.

### Prioridad 1 — métricas y robustez

- métricas agregadas de capture/process/write/preview;
- frame age y dropped-frame counters;
- Job Object para sidecars;
- heartbeat y deadlines;
- hotplug/recovery state machine;
- readback después de controles físicos;
- debounce/coalescing de sliders.

### Prioridad 2 — GPU

- prototipo D3D11 de ajuste de color;
- LUT 3D shader;
- lens map shader;
- conversión NV12/BGRA medida;
- textura compartida entre host y fuente virtual;
- preview DirectComposition opcional.

### Prioridad 3 — funciones visuales

- curvas;
- crop/rotate;
- sharpen/blur;
- calibración OpenCV fuera del hot path;
- perfiles por resolución/lente;
- scopes opcionales.

### Prioridad 4 — laboratorio IA

- interfaz backend;
- modelos candidatos con licencias claras;
- benchmarks por hardware;
- scheduler latest-frame;
- fallback automático;
- UI sólo después de superar el gate.

### Prioridad 5 — release público

- matriz hardware;
- soak tests;
- threat model;
- fuzz;
- SBOM;
- firma Authenticode;
- CI de installer/upgrade/uninstall;
- documentación de contribución y soporte.

---

## 25. Guía corta para otra IA

Si una IA recibe este repositorio, el contexto mínimo correcto es:

> CameraTuner/Webcam-Control es una app colaborativa Windows, open source MIT, con UI Tauri obligatoria. Usa Rust para orquestación/captura/procesamiento, C# sólo para controles DirectShow/property pages y C++ para la Media Foundation Virtual Camera. MediaCapture/MediaFrameReader es el dueño normal de la cámara. Los filtros trabajan en BGRA CPU; preview publica BGRA y usa JPEG por IPC, mientras la salida virtual convierte una vez con libyuv SIMD y publica NV12 por CTFRAME2 triple-slot de capacidad 4K fija. La Media Source v10 consume NV12 directamente y regula el FPS. Existe un lease exclusivo por cámara. No hay AI upscaling todavía. El siguiente trabajo debe priorizar D3D11/HLSL, soak tests y métricas antes de integrar ONNX.

Antes de actuar, la IA debe ejecutar sólo inspecciones no invasivas y respetar si el usuario está usando la computadora. Compilar y leer archivos no implica permiso para manejar ventanas, aceptar UAC, cerrar aplicaciones o controlar la webcam de manera visible.

---

## 26. Documentos relacionados

- `README.md`: entrada rápida.
- `docs/ARCHITECTURE.md`: diseño objetivo y fronteras.
- `docs/RESEARCH-EVALUATION.md`: evaluación de la investigación externa.
- `docs/IMPLEMENTATION-PLAN.md`: fases y gates.
- `docs/adr/0001-native-camera-host.md`: decisión inicial del host nativo.
- `THIRD_PARTY_NOTICES.md`: componentes y licencias.
- `LICENSE.txt`: licencia MIT.

Cuando haya contradicción, el código y las pruebas describen el comportamiento real; luego se debe corregir la documentación. Este archivo debe mantenerse sincronizado con cambios de arquitectura, instalación y pipeline.
