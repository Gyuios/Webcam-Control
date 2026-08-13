# Investigación de implementaciones de cámara

`reference-implementations/` contiene clones locales y superficiales de proyectos externos utilizados únicamente para estudiar arquitectura, rendimiento y compatibilidad. Sus árboles están ignorados por Git y no forman parte del código ni de las dependencias distribuidas por CameraTuner.

- `REFERENCE-ARCHITECTURE.md`: análisis técnico, decisiones y plan de aplicación.
- `reference-lock.json`: origen, revisión exacta, licencia y uso permitido de cada referencia.
- `fetch-references.ps1`: reproduce los clones o verifica que coincidan con el lock.

Desde la raíz del proyecto:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\research\fetch-references.ps1
```

El script no cambia un clon que esté en otra revisión: se detiene para evitar borrar trabajo local accidentalmente.
