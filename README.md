# Control Webcam

Autor: Gyuios  
Copyright © 2026 Gyuios. Todos los derechos reservados.

Aplicación de escritorio para configurar controles DirectShow de webcams en
Windows, creada con Tauri 2, Rust y una interfaz web local.

## Licencia

Este repositorio contiene software propietario. Consulta [LICENSE.txt](LICENSE.txt).
No está permitido copiar, modificar, distribuir ni reutilizar el programa o su
código sin autorización previa y por escrito de Gyuios.

## Requisitos de compilación

- Windows 10 u 11 de 64 bits.
- Node.js LTS.
- Rust estable con el destino `x86_64-pc-windows-msvc`.
- .NET SDK 6 o posterior.
- Microsoft C++ Build Tools y WebView2 Runtime.

## Preparación y ejecución

```powershell
npm install
powershell -ExecutionPolicy Bypass -File .\scripts\prepare-assets.ps1
npm run dev
```

`prepare-assets.ps1` recompila el puente DirectShow y descarga FFmpeg. Estos
archivos no se incluyen en Git para evitar subir binarios y dependencias pesadas.

## Crear el instalador

```powershell
npm run build
```

El instalador NSIS se genera en
`src-tauri\target\release\bundle\nsis\`.

