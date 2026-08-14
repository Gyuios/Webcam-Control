# CameraTuner Filter Plugin SDK v1

CameraTuner admite filtros externos declarativos mediante archivos JSON. La ABI v1 está deliberadamente limitada a transformaciones de color 3×4: ofrece controles personalizados sin cargar DLL nativas ni ejecutar código de terceros dentro del proceso de cámara.

## Instalación local

1. Abre CameraTuner.
2. En **Filtros de software**, pulsa **Plugins**.
3. Copia el manifiesto `.json` en la carpeta que se abre.
4. Detén la vista previa/salida si están activas y vuelve a pulsar **Agregar filtro**. CameraTuner relee y valida el catálogo antes de iniciar el próximo host.

Los manifiestos inválidos se omiten y quedan explicados en el log de diagnóstico. Límites v1:

- 64 manifiestos;
- 256 KiB por manifiesto;
- 32 parámetros por plugin;
- identificadores ASCII de hasta 64 caracteres (`A-Z`, `a-z`, `0-9`, `.`, `_`, `-`);
- sólo números finitos y rangos acotados;
- matriz RGB 3×4, sin acceso al filesystem, red, procesos o memoria externa.

## Matriz

`base` contiene 12 números row-major:

```text
R' = R*m0 + G*m1 + B*m2  + m3
G' = R*m4 + G*m5 + B*m6  + m7
B' = R*m8 + G*m9 + B*m10 + m11
```

La identidad es:

```json
[1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0]
```

Cada `modulation` suma `valorDelParámetro * scale` al coeficiente indicado. Un parámetro puede modular varios coeficientes.

## Estabilidad

El ID del plugin y los IDs de parámetros forman parte de perfiles guardados; no deben cambiar entre versiones. Se pueden agregar parámetros con valores predeterminados. Eliminar o renombrar un parámetro requiere una futura migración de manifiesto.

La evolución prevista añade procesadores GPU firmados/permitidos o WebAssembly aislado. No se usará una ABI de DLL C/C++ sin aislamiento: un crash de plugin no debe bloquear la webcam ni el host.

Consulta `example-warmth.json` y `filter-plugin.schema.json`.
