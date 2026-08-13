# Avisos de terceros

CameraTuner distribuye o adapta los siguientes componentes. El código propio del proyecto se publica bajo la licencia MIT de `LICENSE.txt`.

## Microsoft Windows-Camera VirtualCamera sample

El Media Source de CameraTuner deriva de `Samples/VirtualCamera` del repositorio Microsoft Windows-Camera, revisión `790ac218eba8b6995393e9cc9537dfd7730fdb83`.

Copyright (c) Microsoft Corporation.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

Fuente: https://github.com/microsoft/Windows-Camera

## Tabler Icons

Los iconos SVG de la interfaz proceden de Tabler Icons.

Copyright (c) 2020-2026 Paweł Kuna

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

Fuente: https://github.com/tabler/tabler-icons

## Google libyuv y shiguredo_libyuv

El host de captura usa `shiguredo_libyuv` 2026.1.0 (Apache-2.0), que enlaza una revisión fijada de Google libyuv (`1170363ce55fec2a256ce383479d8a6a3edadffe`) para convertir BGRA a NV12 con las optimizaciones SIMD disponibles en el equipo.

`shiguredo_libyuv` Copyright (c) Shiguredo Inc. y colaboradores. Google libyuv Copyright 2011 The LibYuv Project Authors. Ambos componentes conservan sus avisos y licencias originales.

Fuentes: https://github.com/shiguredo/libyuv-rs y https://chromium.googlesource.com/libyuv/libyuv/

Licencias: Apache License 2.0 y licencia BSD de tres cláusulas de libyuv.

## Tauri y dependencias Rust/JavaScript

Tauri y sus dependencias conservan sus licencias respectivas. Los manifiestos bloqueados `package-lock.json` y `Cargo.lock` contienen las versiones exactas usadas para reproducir la compilación.
