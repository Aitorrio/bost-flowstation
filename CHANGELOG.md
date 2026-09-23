# Changelog — Bost FlowStation → PTBS

Notas para operadores. El dashboard OTA muestra las secciones posteriores a tu versión actual.

## v0.3.35 — Afiliación multipaso ETSI (§16.8.3)

- Si un U-ATTACH trae más de **12** GSSI (p. ej. scan list MXP600), tras el ACK de los 12 la BTS pide **group report** SwMI (EN 300 392-2 §16.8.3) para que el MS re-afilie en varios mensajes (detach-all + amendments).
- Log corregido: ya no dice que el MS reintentará solo. Validar en aire: scan >12 → más de 12 en «Grupos afiliados». Si el MS no multipasa tras el report, limitar scan a ≤12.

## v0.3.34 — Restart recovery en Config

- Interruptor **Restart recovery (proactive)** en Advanced network/timers (config en vivo + perfil TMO Cell), con ayuda «?». Default **off**. Tras Aplicar y reiniciar con el check activo, la BTS re-registra ISSIs cacheados sin tocar el walkie. La recuperación reactiva (al PTT/TG) sigue ON en el motor.

## v0.3.33 — Pending-tail quiet 400 ms

- Ajuste fino: quiet post-drenado Brew **400 ms** (antes 550) tras `GROUP_IDLE`.

## v0.3.32 — Brew: no cortar la última sílaba al soltar PTT remoto

- Tras `GROUP_IDLE` se **aplaza siempre** el `NetworkCallEnd` (aunque el jitter Brew esté vacío) para que UMAC termine de radiar los últimos TCH.
- Quiet post-drenado ~**400 ms** (ajustado en 0.3.33; era 550 / antes 150); evita que el hangtime “se trague” la cola y la suelte al abrir el siguiente PTT.

## v0.3.31 — Brew late entry: audio DL + Hold al salir del TG

- **Audio Brew:** las llamadas de red nuevas abren el circuito en **SwMI** (antes LocalLoopback, pensado para LST; el audio remoto no salía al aire).
- **Cambio de TG mid-QSO:** si el último walkie deja el GSSI, la sesión Brew se **retiene** (Hold) en lugar de End; al volver a afiliar se remonta + D-SETUP.

## v0.3.30 — Fix OTA compile (late entry)

- Visibilidad de `push_control` entre módulos CMCE + match exhaustivo en UMAC para los nuevos SAP de late entry (build release fallaba en 0.3.29).

## v0.3.29 — Late entry usable (QSO a medias)

- **Brew:** si llega GROUP_TX sin walkies afiliados, la llamada se **retiene** (pending) en lugar de tirarse; al primer Affiliate se monta circuito + D-SETUP + audio.
- **Affiliate / cambio de TG:** D-SETUP inmediato si ya hay QSO en ese GSSI (no esperar ~5 s).
- **Despacho LST:** al afiliar un TG con QSO activo, la consola engancha RX (`rx_gssi` verde + audio) sin textos nuevos.
- **`late_entry_supported`** por defecto **true** en `[cell_info]`; checkbox en Config (celda Advanced) junto a System-wide services.

## v0.3.28 — Site trunking suave (estilo DIMETRA / TIP)

- Tras un blip de Brew **ya no** se expulsa a todos los walkies con `D-LOCATION-UPDATE-COMMAND`.
- Al reconectar: resync de suscriptores al core (REGISTER/AFFILIATE); COMMAND solo **bajo demanda** a un ISSI si falla un setup vía Brew en la ventana de soft-recovery.
- Histéresis de backhaul (default **3 s**): blips cortos de 4G/5G no cambian el menú “solo área local”. Las llamadas Brew se liberan al instante; los grupos locales en la celda siguen.
- Nuevo `[brew] backhaul_hysteresis_secs` (0..=60).

## v0.3.27 — TetraPack: no re-registro en la primera GROUP_TX

- Si el core no anuncia versión en el handshake (TetraPack), la primera llamada con mnemonic ya no dispara `BrewReconnected` / `D-LOCATION-UPDATE-COMMAND`.
- Ese barrido solo ocurre tras un disconnect→reconnect real del backhaul (sigue cubriendo PTT denegado tras blip).

## v0.3.26 — OTA: RF OFF al empezar

- Al iniciar una actualización OTA se apaga el SDR de inmediato (antes de compilar), para que las radios pierdan la celda limpiamente. El reinicio final vuelve a abrir RF desde config.

## v0.3.25 — Modal Ubicación LIP (móvil)

- En cada radio solo queda el botón **Centrar** (se quita el texto “Centrar todos” duplicado en la tarjeta).
- El botón superior **Centrar todos** va centrado, más grande y en negrita.
- Título del modal alineado en vertical con el botón de cerrar (móvil y PC).

## v0.3.24 — Fix OTA: préstamo en lip_forward_issi

- Corrige E0716 en `brew_routable` (temporary dropped while borrowed) que bloqueaba el build OTA de 0.3.23.

## v0.3.23 — Reenvío LIP → Brew + dashboard estable

- **Brew:** en Advanced (perfil y ajustes en vivo), **Reenvío de LIP** + **ISSI de destino** justo bajo RSSI export. Cada LIP UL (PID 10) se reenvía a ese ISSI por Brew, digan lo que digan las radios.
- **Dashboard:** el WebSocket ya no ocupa un slot del tope de 32 conexiones HTTP; evita que, pasado un rato, API/WS fallen con timeout y haya que reiniciar.

## v0.3.22 — Ubicación LIP: Centrar todos solo donde toca

- Se quita **Centrar todos** del encabezado del modal.
- En escritorio sigue en la cabecera de la columna de acciones; en móvil, entre el mapa y las tarjetas.
- El **Centrar** por radio no cambia.

## v0.3.21 — Audio LST adaptativo + Geo sin fugas

- **PCM DL:** solo con despacho tomado. En llamada/RX activo vuelve a **80 ms** (latencia); en idle baja a **500 ms**.
- **Ubicación LIP:** al cerrar el modal se destruye el mapa Leaflet (deja de pedir tiles OSM) y no se vuelve a consultar `/api/lst/positions` hasta reabrir.
- Modal Geo: se elimina la barra redundante (Centrar todos + estado ISSI); **Centrar** / **Centrar todos** centrados en la columna de acciones (en móvil, Centrar todos pasa al encabezado).

## v0.3.20 — Dashboard más ligero (Pi)

- Tope de **32** conexiones HTTP(S) concurrentes: evita que el poll agresivo / reintentos tumben el proceso (ERR_CONNECTION_RESET).
- Menos re-renders por RSSI (debounce 250 ms); timers de timeslots 150→250 ms.
- Callsigns / LST status / Geo / service: no martillean la API si la pestaña está en segundo plano o el enlace está caído.

## v0.3.19 — Modal Ubicación LIP

- Título **Ubicación LIP** (antes Geo LIP).
- **Centrar todos** pasa al encabezado de la columna de acciones; en móvil se muestra encima de la tabla (el thead se apila).
- Se elimina Refresh (el modal ya refresca cada 5 s).
- Cierre (×) ya no se superpone con el separador del título en móvil.

## v0.3.18 — Filtro por tipo en Registro SDS

- Selector Todos / LIP / Texto / Estado / Concat / Home / Otros junto a Exportar.
- Se elimina Actualizar: el log se carga al abrir la pestaña y llega en vivo por WebSocket.

## v0.3.17 — Ubicación en Inicio (sin LST)

- Botón **Ubicación** en la tarjeta Radios registrados (Inicio): mismo modal Geo LIP que en LST.
- El almacén de posiciones LIP vive en el dashboard (no depende del perfil LST Dispatch).

## v0.3.16 — Botón Ubicación en roster LST

- El botón **Ubicación** pasa a la tarjeta Radios online (sustituye el Refresh manual, redundante con el poll automático).

## v0.3.15 — Marcador Geo LIP

- Pin de mapa propio (CSS, color accent del dashboard); ya no depende de las PNG rotas de Leaflet/CDN.
- Indicativo RadioID correcto en la tabla Geo.

## v0.3.14 — Geo LIP en despacho LST

- Las posiciones LIP decodificadas (SDS PID 10, UL) alimentan el almacén LST (`note_position`).
- Botón **Geo** en la consola LST: modal con tabla + mapa OpenStreetMap (Leaflet lazy, solo al abrir).
- Se ignoran coords 0,0 (handshake de inicialización). Requiere perfil LST Dispatch activo.

## v0.3.13 — LIP decode + Miura restante

- **LIP:** se decodifican informes cortos (SDS PID 10) a `LIP position: lat, lon` (ETSI TS 100 392-18-1). GeoAlarm/Telegram y el log SDS dejan de ver el payload vacío.
- **Miura (resto):** `mon_pattern` / MPN 1 en channel allocation (PTT largo Sepura); D-RELEASE también por FACCH en el timeslot de tráfico.

## v0.3.12 — Puente OTA hacia PTBS

**Haz OTA una vez** (canal Estable o Beta). Esta versión prepara la migración al producto **PTBS** (*Personal Tetra Base Station*).

- Canal **Estable** pasa a seguir la rama git **`main`** (antes `bost`). La rama `bost` sigue recibiendo este puente para que las instalaciones actuales puedan actualizar.
- OTA reconoce checkout `/opt/ptbs` (preferente) y `/opt/bost-flowstation` (legacy).
- Al instalar el binario se refresca también `/usr/local/bin/ptbs` junto a `bluestation-bs`.
- Anuncio de rebrand en README y mensajes OTA. La marca en UI sigue siendo Bost FlowStation hasta el corte **0.4.0**.
- Ventana de migración: mantén el equipo actualizado; en unos días el repo/ramas/binario completarán el rename a PTBS.

## v0.3.1

- **Grupos afiliados:** el panel del chevron (›) ya no desaparece al instante. Se queda abierto hasta cerrarlo (×, clic fuera, Escape o de nuevo el chevron). Antes lo cerraban el refresco del roster, el scroll y un timer de 3,5 s; el `title` nativo del botón también confundía en móvil.

## v0.3.0

Lanzamiento estable (canal OTA **Estable** / rama `bost`). Consolida el trabajo de la línea 0.2.4–0.2.41: despacho LST en producción, preempt de PTT, dashboard HTTPS canónico y correcciones de campo.

### Actualización desde 0.2.x (estable)

- OTA a **Estable** / `bost` o reinstalar con `install-bost.sh` (no sobrescribe `config.toml` existente).
- Al arrancar, si el dashboard seguía en puertos antiguos (`port = 8080`), se migra a `port = 80` + `https_port = 443`. Abre **`https://<IP>/`**.
- Redirecciones silenciosas en `:8080` / `:8443` se mantienen solo por compatibilidad de marcadores viejos; **instalación e interfaz ya no anuncian esos puertos**.
- Codec de voz LST: si falta, el OTA ofrece rebuild con libtetra-codec (sin SSH).

### Despacho LST (consola local)

- Consola bajo Integraciones: claim de sesión, ISSI despachador, lista de escaneo multi-TG, PTT (ratón/táctil/espacio), SDS, roster, actividad e inbox SDS.
- Llamadas privadas simplex/dúplex (salientes y entrantes); modal/franja de llamada.
- Audio ACELP vía codec OTA; dashboard canónico en HTTPS `:443` (HTTP `:80` redirige).
- **Preempt / interrupción:** PTT LST puede quitar el suelo a un MS local (deny → oferta → Ready al UL quiet); sin teardown de circuito; sin flicker de display Motorola tras Ready.
- Tras PTT de grupo, el dial privado SX/DX ya no queda bloqueado como “Establecida” con el GSSI.
- Modal de llamada entrante solo en el navegador que tiene el despacho tomado (no molesta a otros agentes del dashboard).

### Dashboard / Config / red

- Dashboard HTTPS `:443` + redirect HTTP `:80` (instalador y docs alineados).
- Ayuda «?» en Config (timers TETRA/Brew); whitelist ISSI en perfil Cell; fix TOML con 2+ ISSIs (evita arranque en fallback).
- Wi‑Fi resiliencia (NM drop-in, autoconnect, watchdog) desde 0.2.2–0.2.3.
- Pulidos UX LST/móvil, iconos de navegación, perfiles Cell × Brew.

### Limpieza en 0.3.0

- Eliminado el botón “Abrir consola segura (HTTPS)” del despacho (la UI ya sirve en HTTPS).
- Mensajes de instalador / README / example_config sin publicar `:8080` / `:8443`.

## v0.2.7

- **LST Dispatch:** admit group `NetworkCallStart` without Brew (inbound gate). Join solo selecciona GSSI; PTT abre/cierra la llamada. SDS usa el ISSI del despacho (`source_issi` / `dest_is_group`). Privadas: media ready, duplex UL, errores visibles.

## v0.2.6

- Fix LST PTT borrow-checker errors so `tetra-entities` compiles on OTA.

## v0.2.5

- Fix OTA build: import `CfgLstDispatch` / `apply_lst_dispatch_patch` in tetra-config.

## v0.2.4

- **Despacho LST / LST Dispatch:** consola local bajo Integraciones (mic/altavoz del navegador). Perfil Brew inmutable “Despacho LST”, 1 sesión, XOR con Brew real. Grupo + privadas simplex/dúplex + SDS + roster; codec de voz si el build incluye `asterisk`.

## v0.2.3

- OTA / arranque aplican solos el drop-in NetworkManager `bost-wifi.conf` (powersave off): **no hace falta SSH**.

## v0.2.2

- **WiFi resiliencia:** Disconnect usa `connection down` (ya no inhibe autoconnect). Al conectar se fuerza autoconnect + powersave off en el perfil. Watchdog ligero re-sube un perfil guardado si el enlace cae con la radio WiFi encendida.
- Instalador: drop-in NetworkManager `bost-wifi.conf` (`wifi.powersave=2`). Docs de comprobación en install-and-setup.

## v0.2.1

- Config U-STATUS: en PC los comandos vuelven a una sola fila (código | acción | Quitar); el layout móvil compacto no cambia.

## v0.2.0

Lanzamiento estable con cambios de producto (no solo parches). Canal OTA **Estable** (`bost`).

- **Dashboard móvil:** shell, System/OTA/Setup, tablas, RF/Health, Config, DGNA/Geoalarm/Wi‑Fi e integraciones usables en teléfono (PC sin cambios de layout salvo lo acordado).
- **OTA más robusto:** modal al instante, checks coalescidos y en caché, purge de `.git` vacío, mensajes claros, espera post-reinicio que exige “caída → subida” y lleva a login cuando hay auth (evita SPA “fuera de línea”).
- **Config / perfiles:** timers de Advanced network con reset a default (vacío = default del motor); Live y perfiles Cell comparten la misma semántica de persistencia.
- **Home:** perfiles rápidos Cell × Brew; pulidos de UI (Save al pie, mensajes vacíos, U-STATUS compacto, etc.).

## v0.1.57

- Instalador alineado con OTA: fetch con refspec + reintentos, `reset --hard`, y `ota_channel` según `BOST_BRANCH`.
- README / docs de instalación actualizados (comando curl sigue siendo siempre desde rama `bost`).

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
