# Changelog — Bost FlowStation

Notas para operadores. El dashboard OTA muestra las secciones posteriores a tu versión actual.

## v0.1.56

- OTA: reintentos de `git fetch` ante cortes TLS/red (p. ej. GnuTLS en Pi).

## v0.1.55

- Paso Progreso OTA: estado superior corto, tip distinto bajo la barra y tiempo transcurrido en negrita.

## v0.1.54

- Novedades humanas en la pantalla de actualización (CHANGELOG / Releases).
- El banner de “actualización disponible” abre directamente el resumen de cambios.
- Modal OTA con indicador de pasos más claro (canal → novedades → progreso).

## v0.1.53

- Corrección de compilación en el helper de permisos OTA (`append` / `&str`).

## v0.1.52

- Tras sincronizar el código como root, se ajusta la propiedad de todo el árbol de fuentes
  (no solo `target/`) para que `cargo` como usuario `bts` no falle en `Cargo.lock`.

## v0.1.51

- Al cambiar de canal (p. ej. a Beta), el fetch crea correctamente `origin/<rama>`
  para que la actualización no falle con “unknown revision”.

## v0.1.50

- Diálogo OTA en tres pasos: elegir canal, ver novedades y confirmar, luego progreso.
- El selector de canal se guarda y sigue alimentando el badge / banner automático.

## v0.1.49

- Corrección de un error de compilación en la configuración del canal OTA.

## v0.1.48

- Canales OTA **Estable** (`bost`) y **Beta** (`beta`).
- Sincronización segura con `git reset --hard` (recupera force-push) manteniendo `target/`
  para builds incrementales.
- Si el binario ya coincide con HEAD, no se recompila ni se reinicia en falso.
