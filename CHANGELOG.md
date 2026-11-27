# Changelog — Bost FlowStation → PTBS

Notas para operadores. El dashboard OTA muestra las secciones posteriores a tu versión actual.

English version: [`CHANGELOG_en.md`](CHANGELOG_en.md).

## Sin publicar

- **La asignación de canal de un mensaje descendente fragmentado va en el MAC-END.** Cuando un mensaje con asignación de canal no cabía en lo que quedaba del slot, la estación lo fragmentaba y enviaba la asignación de canal en el MAC-RESOURCE con el primer fragmento. TS 100 392-2 cláusulas 23.4.2.1.1 y 23.5.4.2.1 la exigen en la PDU MAC-END y la prohíben en el MAC-RESOURCE; el marcador de uso se queda en el MAC-RESOURCE. Un D-SETUP de entrada tardía que compartía el slot del MCCH con una respuesta dirigida a una radio (aceptación de registro, confirmación de adjunción de grupos) se fragmentaba así, y la radio no lo seguía: tras un cambio de estación oyendo una llamada de grupo solo entraba con el siguiente D-SETUP periódico (prueba de campo 27-11: D-SETUP repetido 0,6 s y 1,0 s tras el primer contacto, ambos fragmentados, radio aún en el MCCH 4,9 s tras el primer contacto, el D-SETUP periódico, sin fragmentar, siguió a los 5,4 s). La asignación de canal pasa ahora al MAC-END, cifrada con la TM-SDU cuando la PDU va cifrada (EN 300 392-7 cláusula 6.7.1.2).
- **Aviso «network call start changed brew_uuid» pasa a depuración.** Cada turno de palabra de una llamada de red tiene su propio UUID de Brew; un turno nuevo sobre un circuito mantenido durante el hangtime es el caso normal y ya no se registra como aviso (llegaba como alerta de estación por Telegram en cada cambio de hablante).
- **Cambio de estación oyendo una llamada de grupo: reincorporación inmediata otra vez.** Una radio que llega a una celda durante una llamada de grupo se registra primero por la orden de la estación (D-LOCATION UPDATE COMMAND), hace después su propio registro de roaming y vuelve a adjuntar sus grupos (TS 100 392-2 cláusula 16.4.1.1 a), 16.8.2); ignora el D-SETUP de entrada tardía enviado antes de eso. Desde la fusión de SC3 (v0.5.x, 9 de noviembre) MM ya no comunicaba a CMCE un grupo que la radio vuelve a adjuntar sin cambio, así que el D-SETUP no se repetía y la radio quedaba fuera de la llamada hasta el D-SETUP periódico, hasta 5 s después (prueba de campo 26-11: 5,1 s en vez de 0,9 s). MM vuelve a comunicar esos grupos, solo a CMCE; el servidor Brew sigue sin recibir una afiliación duplicada.
- **Llamada en curso: acceso aleatorio sin petición de capacidad confirmado.** En el tiempo de espera de una llamada (hangtime), una radio oyente que solo confirma un mensaje del enlace descendente envía un BL-ACK aislado por acceso aleatorio en el canal de tráfico, sin petición de capacidad; a menudo es un acceso iniciado en el MCCH cuya confirmación la radio no vio porque el D-SETUP la envió antes al canal de tráfico (cláusula 23.5.1.4.11). La estación solo confirmaba los accesos aleatorios con petición de capacidad, así que la radio repetía el acceso cinco veces (TS 100 392-2 cláusulas 23.5.1.4.8 y 23.5.1.4.12) y cada copia salía como aviso «received unexpected ACK» (también en Telegram). Ahora la estación lo confirma con el indicador de acceso aleatorio en un MAC-RESOURCE propio mientras el enlace descendente de la llamada está en señalización (hangtime); el acceso reservado (cláusula 23.5.2) sigue sin confirmación. Además, un ACK sin mensaje pendiente de confirmación ya no es un aviso (es normal según la cláusula 22.3.2.3 j)), ni lo es el cambio del circuito de una llamada terminada por la nueva llamada en el mismo slot («Circuit already exists»). Visto en dos estaciones el 2026-11-26. No probado en el aire.
- **Telegram: «Info IP del sistema».** Nueva categoría en Alertas de Telegram (`[telegram_alerts] alert_system_ip`, desactivada por defecto). Cuando la estación está en línea tras arrancar envía una vez el nombre de host, la dirección LAN con su interfaz, la dirección pública (WAN) y la red Wi-Fi, y de nuevo cada vez que una de ellas cambia («red cambiada»). Tras arrancar o tras un cambio espera hasta 2 min a la dirección WAN; si no llega, envía el mensaje con «n/a». La dirección WAN se consulta en `ifconfig.me` (con `api.ipify.org` como respaldo) solo mientras la categoría está activada, al arrancar y luego cada 5 min. Sustituye a los scripts propios de «IP al arrancar» en la Raspberry Pi.
- **Apagar y encender SDRs adicionales en vivo (página Celdas).** Cada SDR sin la celda principal tiene ahora un interruptor «Encendido/Apagado» en la página Celdas. Apagar escribe `enabled = false` en sus `[[cells]]` (los ajustes se conservan) y se aplica al momento sin reiniciar: solo se cierra ese SDR, la estación, sus otras celdas, el dashboard y los enlaces de red siguen funcionando. La celda se retira como una celda quitada en vivo: libera sus llamadas; si otra celda de la red enlazada sirve su área de localización, sus terminales conservan el registro y esa celda sigue llevando sus llamadas de grupo (reseleccionan allí sin registrarse, TS 100 392-2 cláusulas 16.4.1.0 y 18.3.4.7), si no, los da de baja hacia la red; a los 2 s el hilo para y cierra el SDR. Encender quita `enabled = false`, comprueba que la celda sigue cabiendo (portadora libre, ajustes del SDR completos), abre solo ese SDR y la celda se une al conmutador de sitio en marcha. Los SDR apagados aparecen aparte («Apagados») con sus celdas. Un SDR compartido por varias celdas se apaga y enciende entero (el SDR se abre una vez para todas); una celda en el SDR de la celda principal no se puede apagar sola. Una celda apagada cuenta para el enlace de red de la celda principal, así que apagar o encender la única celda adicional ya no cambia el cableado (antes, quitarla pedía reinicio); por eso una estación con una celda apagada y Brew arranca con el conmutador de sitio. Si el cambio no se puede aplicar en vivo (p. ej. encender una celda mientras el puente Asterisk sirve varias celdas), la página indica el motivo y pregunta antes de reiniciar. Corregido además: al quitar en vivo una celda de la misma área de localización, la celda principal soltaba las llamadas de grupo que llevaba para las radios de esa celda. API: `POST /api/cells/enable` (`{"ids": [...], "enabled": bool, "allow_restart": bool}`). Probado en el aire el 2026-11-25: apagado y encendido otra vez sin problemas.

## v0.6.9 — Configuración en caliente

- **Los cambios de configuración se aplican sin reiniciar.** "Aplicar y reiniciar" (ajustes en vivo, TOML en bruto, perfiles Cell/Brew) ahora llama primero a `POST /api/config/reload`: lee el `config.toml` guardado y, si solo cambia algo que se puede aplicar en caliente, lo carga en todas las celdas y no reinicia (UMAC reconstruye SYSINFO y el cifrado; el resto de entidades ya leía la configuración en cada uso). También se aplican en caliente: el enlace de telemetría y el de control (Fleet Control) se reconectan con los nuevos datos o se cierran si se quita la sección, el monitor de salud y el watchdog, las alertas de Telegram, el puente Asterisk (reabre el socket SIP y se registra de nuevo cuando no hay llamadas; si arrancó apagado, encenderlo pide reinicio), las reglas de registro de `[security]` (lista blanca, límites, autenticación, claves de abonados: se aplican desde el siguiente registro de cada radio) y el área de localización (ver abajo). El flujo de telemetría ahora siempre existe, aunque no haya consumidores. Brew: el enlace de cada red se reconecta con el nuevo servidor, credenciales y temporizadores. LST: encender/apagar la consola y cambiar el ISSI del operador (si arrancó apagado, encenderlo pide reinicio). `debug_log` abre, cambia o cierra el fichero de log. `service_name` se cambia. Dashboard: usuario, contraseña y vista pública. Si el cambio toca algo que no se puede aplicar en caliente no se aplica nada y se reinicia como antes, indicando el motivo: SDR y hardware (`phy_io`), `stack_mode`, portadora y plan de frecuencias (reajustan el SDR), celdas adicionales y sus SDR, con varias celdas, cualquier cambio en las redes (añadir, quitar, qué celda sirve a cuál, ganar o perder el enlace Brew), el colour code y MCC/MNC, el cifrado de la interfaz aire (clase de seguridad, claves), y la escucha del dashboard (dirección, puertos, `source_dir`: un puerto mal puesto dejaría sin acceso a la gestión). No probado en el aire.
- **Conformidad ETSI restablecida en la recarga en caliente.** Revisado frente a TS 100 392-2 V3.10.1 y EN 300 392-7 V3.5.1 y corregido donde la primera versión no seguía la norma: (1) La clase de seguridad y las claves ya no cambian en silencio: cambiarlas con radios registradas requiere D-CK CHANGE DEMAND en un slot acordado con SYSINFO a la vez (EN 300 392-7 cláusulas 4.5.5.6 y 4.5.6), así que hasta que la recarga lo haga reinician la estación. (2) El colour code y MCC/MNC también reinician: forman el colour code extendido del scrambling (TS 100 392-2 cláusulas 8.2.5.2, 23.2.1); un cambio hace que todas las radios pierdan la celda y se registren como en otra red (cláusula 16.4.1.0), lo que dejaba registros y sesiones Brew obsoletos. (3) Un cambio en caliente del área de localización pide ahora a cada radio registrada que se registre de nuevo con D-LOCATION UPDATE COMMAND, una radio por trama TDMA (cláusula 16.4.3); una radio que se queda en la celda no hace ninguna reselección que la obligue a registrarse en la nueva LA. (4) Se detecta una contraseña o clave cambiada: la comprobación comparaba la forma censurada del log, así que una nueva contraseña de Brew, telemetría, control o Asterisk no reconectaba el enlace. (5) Brew: cuando una recarga cambia lo que anuncian las celdas (área de localización, vecinas, valores de difusión), la estación vuelve a enviar su BTS info por la misma conexión, así las ubicaciones de radios y el directorio de celdas del servidor se actualizan sin reconectar (los servidores con `bts-info` ya toman un BTS info repetido como la nueva lista de celdas; los servidores Brew simples no ven nada nuevo).
- **Red de una sola celda: ganar o perder el enlace Brew en caliente.** El router acepta cambios de entidades entre dos ticks (`RouterMailbox`): al añadir `[brew]` se crea la entidad Brew y se conecta; al quitarlo se retira. Las redes que ninguna celda sirve son solo configuración. No se aplica si LST ocupa el mismo hueco.
- **Brew: un cambio en caliente espera a las llamadas de red en curso.** Un cambio guardado del enlace Brew (servidor, acceso, temporizadores) ya no corta las llamadas de red en curso: la estación espera a que no pase ninguna llamada por el enlace durante 5 s, como mucho `[brew] live_wait_secs` (60 por defecto, de 0 a 600), y luego reconecta igualmente. `live_apply_immediately = true` reconecta al momento. Ambos en Config → Brew → Brew avanzado.
- **Ganancias, antenas y corrección ppm del SDR en caliente.** Un cambio guardado de `rx_gains`/`tx_gains`, `rx_ant`/`tx_ant` o `ppm_err` (celda principal y celdas adicionales; en un SDR compartido por varias celdas, los valores de la primera celda como al arrancar) se aplica al SDR en marcha sin reiniciar. Un cambio de ppm resintoniza RX y TX y reconstruye el canalizador para las frecuencias corregidas de las portadoras manteniendo los contadores de muestras, así la celda sigue en el aire y a tiempo (como mucho se pierde un bloque de un slot). Quitar una ganancia o una antena, el dispositivo, la frecuencia de muestreo, los canales y las frecuencias centrales siguen pidiendo reinicio.
- **Código de color, MCC/MNC y portadora en caliente reconstruyendo solo esa celda.** Cambian la identidad de la celda en el aire: todas sus radios pierden la celda y, al encontrarla de nuevo, se registran como en otra red (TS 100 392-2 cláusulas 8.2.5.2, 23.2.1, 16.4.1.0). En lugar de reiniciar la estación, la celda se reconstruye en su sitio: MM da de baja primero cada una de sus radios hacia la red (grupos y luego el registro), descarta los registros y el estado de claves de la identidad anterior, CMCE libera las llamadas de la celda, UMAC/LMAC pasan al nuevo scrambling y difusión, el servidor Brew recibe la nueva lista de celdas y las radios se registran de nuevo. Las demás celdas, el dashboard y los enlaces de red siguen funcionando. Un cambio de portadora funciona para una celda con un SDR propio (el SDR se resintoniza como en un cambio de ppm y el conmutador de sitio enruta por la nueva portadora); con un SDR compartido, otra frecuencia de muestreo o al añadir o quitar una segunda portadora sigue pidiendo reinicio. Las páginas Celdas e Infraestructura también guardan ahora a través de la recarga en caliente («Guardar y aplicar») y solo reinician cuando hace falta.
- **Celdas añadidas y quitadas en vivo.** Una celda con SDR propio que sirve la red de la celda principal ahora arranca y para sin reinicio (página Celdas, TOML, perfiles). Una celda nueva tiene su propio hilo y SDR y se une al conmutador de sitio en marcha; sus hermanas la anuncian como vecina. Una celda quitada primero libera sus llamadas; si otra celda de la red enlazada sirve su área de localización, sus terminales conservan el registro (reseleccionan allí sin registrarse, TS 100 392-2 cláusula 16.4.1.0, y el conmutador los cuenta en esa celda), si no, los da de baja hacia la red primero; a los 2 s la celda para y cierra su SDR. Sigue reiniciando: una celda en un SDR compartido, una celda de otra red, añadir o quitar la celda que hace que las celdas empiecen o dejen de compartir el enlace de red (de una a dos celdas o al revés), añadir una celda mientras el puente Asterisk sirve varias celdas, y hacerlo junto con ganar o perder el enlace Brew.
- **Cambio de clase de seguridad y de clave de cifrado en vivo, anunciado a las radios.** Un cambio guardado de la clase de seguridad (1, 2, 3) o de la clave de la celda (clase 2: número/versión/clave SCK, clase 3: CCK e id de CCK) ya no reinicia la estación. La celda sigue con la configuración anterior y envía D-CK CHANGE DEMAND a cada radio registrada, también a las que están en claro, indicando el slot del cambio: el primer slot de la multitrama al menos `[security] change_lead_secs` después (ajuste nuevo en la página Seguridad, 5–300 s, por defecto 30) (EN 300 392-7 cláusulas 4.5.5.0, 4.5.5.6, 4.5.6.1 a 4.5.6.3; a clase 1 con tipo de cambio de clave "sin clave de cifrado"). En ese slot SYSINFO y el cifrado cambian juntos en el enlace descendente, el ascendente dos slots después (cláusula 4.5.5.4), y unos slots más tarde el cambio se escribe en la configuración en marcha de la celda. Según la cláusula 4.5.6.0 las radios registradas en claro se registran de nuevo cuando la celda pasa a cifrado, las radios sin la clave nueva se registran de nuevo para obtenerla y, en un cambio a clase 1, las radios cifradas siguen registradas en claro. Cada celda anuncia su propio cambio. Mientras un cambio está anunciado se rechaza otro distinto hasta que entra en vigor; uno aún no anunciado se puede sustituir o retirar. El algoritmo TEA de una celda que sigue cifrada sigue reiniciando. El botón «Reiniciar estación» de la página Seguridad pasa a ser «Aplicar» (en vivo cuando se puede, si no reinicio) y muestra un cambio pendiente.
- **Fleet Control: configuración aplicada en vivo.** Una configuración que envía Fleet Control (`SetConfig`) y que la estación puede tomar sin reinicio se aplica ahora en vivo, como al guardar en el dashboard; un cambio que necesita reinicio sigue reiniciando. La estación anuncia la nueva capacidad `config-live-v1` y responde a ese cambio con el código `applied_live`. La ventana de confirmación y la vuelta atrás automática siguen como antes y cuentan desde que se aplica el cambio; un servidor sin la capacidad confirma con el hello que la estación vuelve a enviar.

## v0.6.8 — Versión de mantenimiento

- **Sin cambios funcionales.** Solo sube el número de versión.

## v0.6.7 — Página Celdas y varias celdas en un SDR

Varias celdas en un SDR: el motor multicelda (`SdrHub`) y la seguridad por celda los construyó Yiannis Samouhos (ysamouhos); la implementación en el dashboard (página Celdas) y la reestructuración de la configuración son de Sebastian, OE4SXM.

- **Multi Network: una red que ninguna celda usa puede quedarse en la configuración.** Se conserva pero inactiva (sin enlace Brew, no se enruta nada) hasta que una celda la elija; la tarjeta Redes la marca «(sin usar)». Hasta ahora la comprobación la rechazaba («no cell serves this network»), así que no se podía preparar una red de antemano ni conservarla al mover su celda. Fleet Control: la estación anuncia `networks-unused-v1`, y el editor de configuración del servidor Brew solo permite una red así en las estaciones que lo anuncian.
- **Página Celdas — provisional, sin validar en el aire.** Una página (menú *Celdas*, siempre visible) para todas las celdas de la estación, también la principal, agrupadas por el SDR que las lleva. Sustituye a la página SDR y celdas y su interruptor (`[dashboard] sdr_manager` ya no se usa; una configuración antigua con él sigue cargando) y a las partes de añadir / editar / quitar de la tarjeta Celdas de Infraestructura, que ahora solo lista las celdas con un enlace. Para cada SDR se elige el modo: *una celda*, *multiportadora* (una celda en dos portadoras, `secondary_carrier`) o *multicelda* (varias celdas en el mismo SDR); los modos se combinan entre SDR y una celda multicelda puede tener además una segunda portadora. Por SDR: dispositivo (botón Escanear), frecuencia de muestreo, corrección ppm, antenas y ganancias (*Ajustes del SDR*). Cada celda es una tarjeta con sus frecuencias, portadoras, código de color y área de localización, red (MCC/MNC), cifrado y estado en vivo; al pulsarla se abre un diálogo con cuatro pestañas: *Radio* (SDR, portadora principal y segunda, código de color), *Red y área* (red, las redes sin usar marcadas; área de localización, vacío = como la celda 0), *Cifrado y acceso* (ajustes propios sí/no, clase 1/2/3, KSG, número/versión/clave SCK, CCK e id, grupos en claro, lista blanca de ISSI y su modo, autenticación, autenticación mutua; para la celda 0 el `[security]` de la estación) y *Vecinas* (la misma lista y editor de vecinas que en la página Infraestructura, también rellenar desde la lista de celdas del servidor Brew, se guarda aparte; los valores de reselección siguen en Infraestructura). Una tarjeta *Comprobación* muestra una clase de seguridad por red, la clase 3 sola en su red, celdas de una red con áreas de localización distintas, redes sin usar y la comprobación del servidor de toda la configuración; otra tarjeta enlaza los ajustes de toda la estación (Seguridad, Infraestructura, Config). Las claves nunca se muestran; un campo de clave vacío conserva la del archivo. Las frecuencias centrales se calculan solas (en medio de las portadoras); al guardar se valida toda la configuración y la estación se reinicia. Se escribe el formato de siempre (las celdas que nombran el mismo `device` comparten el SDR), así que perfiles, Dual Carrier y Fleet Control siguen funcionando. Config → RF ya no muestra código de color, área de localización ni Dual Carrier (están en la página Celdas), y la tarjeta Dual Carrier de la página de inicio se llama ahora *MultiCell / DualCarrier*, muestra el número de celdas y su *Configurar…* abre la página Celdas; la página RF elige un SDR en lugar de una celda («SDR #1 (Celda 0/1)»), porque las celdas de un mismo SDR comparten sus valores (antes una segunda celda en el mismo SDR se quedaba en «esperando»); la cascada de RF vuelve a dibujarse (escritorio y móvil: desde la vista RF por celda leía un búfer vacío hasta elegir otra celda); la página Seguridad indica que el cifrado por celda se ajusta allí. `GET /api/sdrs`, `POST /api/sdrs/check`, `POST /api/sdrs` (ahora con `network` y `security` por celda); `POST /api/sdrs/enabled` desaparece.
- **Config → RF: campo «Sample rate (MHz)».** En «Hardware RF», junto a PPM. Vacío = el valor por defecto del driver, que se muestra en el campo: Pluto+ 1 MHz, SXceiver/MuCell 0,6 MHz, LimeSDR/USRP 0,512 MHz. Antes el panel suponía 0,6 MHz para todos los SDR, por lo que Dual Carrier dejaba elegir menos portadoras de las que caben en un Pluto+, y la página SDR y celdas no podía activar multiportadora si `sample_rate` no estaba en el archivo. Guardar en Config ya no borra `tx_center_freq`/`rx_center_freq` ni `sample_rate` cuando Dual Carrier no está activo (los usan las celdas que comparten un SDR); solo al apagar Dual Carrier en la misma página se quitan como antes.
- **`[cells.security]` — provisional, sin validar en el aire.** Una celda adicional puede tener su propio cifrado de la interfaz aire y control de acceso: el `[security]` de la estación con las claves de esa celda encima. `aie` (clase, KSG, SCK/SCKN o CCK, `clear_groups`) sustituye por completo el `[security.aie]` de la estación (`aie = { class = 1 }` deja la celda en claro); `issi_whitelist`, `whitelist_mode`, `authentication` y `mutual_authentication` se sobrescriben uno a uno. La base de claves de abonados, E2EE, el límite del registro, el límite de ritmo y la política de baja siguen siendo de toda la estación y se rechazan por celda. Las celdas de una misma red deben tener la misma clase de seguridad (una celda en claro junto a una cifrada necesita su propia red en `[networks]`); la misma clase con claves distintas es válida. La clase de seguridad 3 ya no se rechaza con varias celdas siempre que la celda SC3 esté sola en su red (antes se rechazaba con cualquier `[[cells]]`). Pendiente: reglas para llamadas y reselección entre celdas con claves distintas. Diseño en `Docs/multi-cell-plan.md`, fase 7b.
- **Dashboard: editar los ajustes y la seguridad de una celda.** Se hace en la página Celdas (ver arriba), para las celdas adicionales y la principal. `GET /api/cells/settings?id=N` y `POST /api/cells/update` se quedan para scripts.
- **Varias celdas en un SDR — provisional, sin validar en el aire.** Las celdas que nombran el mismo `soapysdr.device` ahora comparten ese SDR en lugar de ser rechazadas: cada una conserva su código de color, área de localización, red y lista de vecinas (a diferencia de `secondary_carrier`, que es una celda con dos portadoras), y Brew las ve como celdas distintas. El SDR se abre una sola vez para todo el grupo; las celdas avanzan al compás de su único reloj de muestras y una celda que deja de responder se sustituye por silencio en su portadora a los 250 ms para que no bloquee a las demás. Todas las celdas del grupo repiten los mismos `sample_rate`, `tx_center_freq`, `rx_center_freq` y `ppm_err`, y todas las portadoras deben caber en la misma banda (se comprueba al arrancar). Las ganancias y antenas salen de la primera celda del grupo; la ganancia de TX es común. La tarjeta Celdas muestra «SDR … (compartido con la celda N)». Diseño en `Docs/multi-cell-plan.md`, fase 7.
- **Inicio: clase de seguridad y algoritmo junto a cada celda.** La cabecera de la celda muestra «SC2 · TEA2» (o «SC1 · abierta») según los ajustes de esa celda, también con una sola celda. `GET /api/cells` incluye `security` por celda (`class` y, en una celda cifrada, `ksg` 1–4 = TEA1–TEA4).
- **Diálogo de celda: la red de la propia celda ya no aparece como «sin usar».** La lista de redes de la pestaña *Red y área* solo contaba las otras celdas.
- **Fleet Control: interruptor de RF.** El servidor Brew (1.22.9) puede apagar el transmisor con la función Suspender de la estación (`SetRfStandby`, capacidad `rf-standby-v1`) y volver a encenderlo (reinicio del servicio). Mientras tanto el dashboard y Fleet Control siguen accesibles; el hello informa `rf_standby` y se envía de nuevo cuando cambia, también tras un Suspender en el dashboard, así que el servidor muestra la estación en *Standby* en lugar de desconectada. Las métricas dan las celdas como `standby`, así que no salta ninguna alarma del SDR. Los servidores antiguos ignoran el campo.
- **Instalador: funciona en Debian 12 bookworm y 13 trixie (Raspberry Pi OS).** Una versión de Rust ya instalada en el home del usuario del servicio y demasiado antigua para este código (la edición 2024 necesita Rust 1.85) se actualiza ahora a la estable actual antes de compilar; antes el instalador la dejaba y la compilación se paraba con «failed to parse manifest» (visto en Pis que ya tenían Rust instalado, normalmente con bookworm). El instalador también instala `sudo` si falta (un Debian sin él no podía compilar como usuario del servicio). Comprobado con una instalación completa en Debian 12 y 13, una vez con un Rust 1.80 antiguo ya instalado.
- **Página Infraestructura: campo de certificado CA para telemetría y control remoto.** Las dos tarjetas de enlace tienen un campo *Certificado CA (opcional)* (`ca_cert`, p. ej. `/etc/ssl/certs/ca.der`) para un servidor cuyo certificado viene de tu propia CA; vacío si no se usa, y entonces se confía en las CA del sistema como antes. Al guardar se comprueba que TLS está activado y que el archivo existe y es un certificado DER o PEM. Ahora también funciona un archivo PEM con varios certificados (antes solo un certificado DER). Un archivo CA que falta o está roto ya no para la estación al arrancar (antes salía y se reiniciaba en bucle): se registra el error, ese enlace no puede conectar y la radio sigue funcionando.

## v0.6.6 — TEA4 completa el conjunto A de cifrado TETRA

- **Cifrado de interfaz aérea TEA4.** El cuarto generador de secuencia cifrante de ETSI TS 104 053-1 (cláusula 8) está implementado registro a registro como TEA1–TEA3: registro de salida de ocho bytes, registro de clave de siete bytes en el que la clave de 80 bits se mezcla byte a byte, la sustitución P, el expansor, f1/f2 y BP de las figuras 19–24, máscara de IV 56DC28E3, 35 pasos de arranque y un byte de secuencia cada 19 pasos. `security.aie.ksg = 4` hace funcionar ahora una celda de clase 2 o 3 con TEA4 en lugar de caer a claro, y la página Seguridad ofrece TEA4 en el selector de algoritmo. TEA4 es el algoritmo de exportación que llevan muchos terminales comerciales (Hytera entre ellos), a los que esta estación hasta ahora no podía servir cifrados.
- **Verificación.** Las tablas se comprueban contra los propios ejemplos de la especificación (P(28) = D4, el ejemplo de carga del IV) y sus propiedades estructurales (P es una permutación, cada caja S está equilibrada); no existen vectores públicos de secuencia TEA4, así que el primer registro en el aire de una radio TEA4 es la prueba de interoperabilidad. Por favor, informa de ella.

## v0.6.5 — Las llamadas de grupo ya no se cortan si la radio repite su U-SETUP

- **El que habla conserva la palabra cuando su U-SETUP llega dos veces.** Si la radio no recibe el acuse de capa 2 de su U-SETUP, lo repite (el basic link no suprime duplicados, TS 100 392-2 cláusula 22.3.2.3 NOTA 3). La estación trataba la repetición como entrada tardía en la llamada que la propia radio acababa de abrir y le enviaba un D-CONNECT con "concedido a otro usuario": la radio dejaba de transmitir y cada llamada terminaba a los 3 s por inactividad del enlace ascendente sin un solo frame de voz (visto en un registro de campo con dos radios). Ahora la repetición recibe otra vez el D-CONNECT de la llamada original (transmisión concedida, propiedad de la llamada, cláusula 14.5.2.1.2), sin D-CALL PROCEEDING y sin cambiar la palabra.

## v0.6.4 — Grupos DGNA al servidor Brew también en beta

- **`main` fusionado en `beta`.** El informe DGNA al servidor Brew (v0.5.19, hasta ahora solo en `main`) está ahora también en `beta`; ver la entrada v0.5.19 más abajo. No cambia nada más.

## v0.6.3 — El registro con autenticación y cifrado vuelve a funcionar en Motorola

- **Sin elemento Security downlink en el D-LOCATION UPDATE ACCEPT.** Desde v0.5.15 el accept de un registro cifrado llevaba un elemento "Security downlink" relleno con los 10 bits de los parámetros de cifrado. EN 300 392-7 cláusula A.7.3 (tabla A.35a) define ese elemento como el resultado de autenticación R1 más indicadores de petición de información, y la SwMI solo puede enviarlo a una radio que se sabe que lo soporta (cláusulas 4.4, 4.4a). En una celda de clase 3 con TEA2 su primer bit decía R1 = 0 ("autenticación fallida") junto a R1 = 1 en el Authentication downlink. Por eso las radios Motorola ya no se registraban con autenticación después de v0.5.10; con la corrección dos radios Motorola vuelven a registrarse en las clases de seguridad 1, 2 y 3 (probado en el aire). El accept lleva otra vez el resultado solo en el Authentication downlink, como hasta v0.5.14.
- **Ya no se cifra por iniciativa de la red un registro en claro.** Desde v0.5.15 una celda de clase 2 cifraba una radio que se registraba con cipher control desactivado si indicaba capacidad SCK. EN 300 392-7 cláusula 6.6.2.1 no contempla ese caso: cipher control desactivado es un registro de clase 1 (en claro) y la radio no espera enlace descendente cifrado. Esa radio vuelve a quedar en claro. El elemento de seguridad en el SYSINFO (v0.5.15) se mantiene.
- **Sin segundo desafío justo después de un registro autenticado.** Las radios Motorola siguen a un accept de actualización por demanda con su propia actualización de itinerancia y no responden a un segundo desafío, de modo que esa actualización quedaba retenida hasta que vencía T354 y nunca se contestaba (visto en SC1, SC2 y SC3). Una actualización posterior que llega cifrada con la clave recién acordada en 30 s (un T354), o en claro en 5 s, se acepta ahora sin nuevo desafío; las actualizaciones en claro posteriores se desafían como antes. La autenticación en el registro la inicia la SwMI (EN 300 392-7 cláusula 4.4.2) y no es obligatoria en cada actualización.

## v0.6.2 — El cifrado SC2/SCK ya no espera al informe de grupo (MTH800 / MTP850)

- **Los terminales SC2/TEA2 (con SCK preaprovisionada) ahora cifran con solo el accept de actualización de ubicación.** El cifrado de interfaz aérea de una radio de clase de seguridad 2 se retenía hasta que su intercambio de informe de grupo terminara *y* llegara un ACK LLC de enganche final, incluso después de que el propio `D-LOCATION-UPDATE-ACCEPT` ya hubiera sido reconocido. Si el informe de grupo seguía en vuelo, la transición de cifrado pendiente expiraba a los 30 s («no recibió el ACK LLC / la finalización del informe de grupo requeridos», con el estado TX ya *Acknowledged*) y la radio encadenaba reenganches ItsiAttach repetidos. Los terminales SCK antiguos (MTH800 / MTP850) caían en esto de forma habitual. Ahora SC2/SCK se activa en cuanto su propio accept es reconocido por LLC; el informe de grupo se completa de forma independiente. La transmisión por sí sola sigue sin bastar: el accept debe ser reconocido.
- **Solo la transición en claro (sin cifrado) sigue esperando al informe de grupo.** La activación SC3/DCK (ligada al resultado de autenticación) y la ruta en claro no cambian; esto restringe la espera del informe de grupo únicamente al caso en claro. Complementa la ventana de gracia del ACK de enlace básico de v0.6.1: aquella evita que los ACK tardíos se descarten en el LLC, esta evita que MM acople en exceso la activación SC2 a un intercambio no relacionado.

## v0.6.1 — Ventana de gracia para el ACK del enlace básico (radios en economía de energía y terminales antiguos)

- **Audio de grupo y registro estables para radios en economía de energía y terminales antiguos (MTH800 / MTP850 / Hytera).** En modo reconocido (enlace básico), la estación reintentaba una PDU durante la ventana ETSI T.251/N.252 (~0,9 s en portadora única) y, al agotarla, descartaba el BL-ACK que llegaba después como «ACK inesperado». Las radios en economía de energía (y algunos terminales antiguos) reconocen una o dos tramas más tarde, así que su ACK siempre llegaba tarde y se perdía: las transacciones de cifrado y de registro de MM quedaban sin confirmar, forzando un reenganche (ItsiAttach) constante que terminaba en «reselección de celda no declarada» y la caída del audio de grupo. Ahora, al agotar los reintentos en el aire, la entrada **permanece viva sin emitir** durante una ventana de gracia (una multitrama ≈ 1,02 s, más el ciclo de monitorización de la radio si duerme) para aún aceptar ese ACK tardío antes de declarar la PDU perdida. Las radios MTP3000/6000, que reconocen dentro de la ventana, no cambian de comportamiento.
- **La ventana de gracia se ajusta al modo de economía de energía concedido.** MM refleja el ciclo de economía de energía concedido (en tramas; `None` = siempre despierta) en el registro de abonados compartido, y el LLC lo suma a la ventana base, de modo que una radio que sólo despierta periódicamente dispone de más margen para reconocer.

## v0.6.0 — WireGuard en la pestaña Red y gráficas de los sensores del sistema

- **WireGuard desde el panel (pestaña Red).** Nueva tarjeta para configurar el túnel sin editar archivos: interfaz (dirección, puerto de escucha, **MTU**, DNS, clave privada) y peers (clave pública, endpoint, **IPs / redes permitidas**, keepalive persistente, clave precompartida), con estado en vivo (último handshake, bytes recibidos y enviados), arrancar/parar el túnel, iniciar con el sistema y reinicio automático al guardar. Usa `/etc/wireguard/wg0.conf` (modo 0600) y `wg-quick`; necesita `wireguard-tools` y ser root o tener `sudo` sin contraseña. Las claves son de solo escritura (el panel nunca las devuelve), todos los valores se validan antes de escribirlos y no se sobrescribe una configuración con directivas que el editor no gestiona (`PostUp`, `Table`…) sin confirmación. Un solo túnel, `wg0`.
- **Gráficas en el tiempo de los sensores del sistema.** La pestaña Sistema muestra una gráfica de la potencia total (con mín/media/máx) y una minigráfica en cada sensor (temperaturas, tensiones, corrientes, potencias) de los últimos ~20 min. El historial se guarda en la estación, así que al recargar la página o conectar otro navegador las gráficas ya vienen llenas.

## v0.5.19 — El servidor Brew sabe qué grupos se dieron por DGNA

- **Grupos DGNA al servidor Brew.** Con un servidor que lo ofrece en el canal de control (extensión `dgna-report`, brew-server 1.21.9 o posterior con `[control]`), la estación informa por el canal de control de cada asignación y retirada DGNA, también de las hechas en su propio dashboard, y tras cada conexión envía todos los grupos DGNA que conoce. Así el servidor guarda qué grupos tiene cada radio (también tras un reinicio) y puede retirarlos más tarde. No cambia nada en el interfaz aire y no depende del interruptor de Fleet Control. Un servidor sin `dgna-report` no recibe nada nuevo. Aparece en la página Infrastructure, "Server features", bajo grupos. Ver `Docs/fleet-control.md`.

## v0.5.17 — Las radios Motorola vuelven a registrarse en celdas sin cifrado

- **Las celdas sin cifrado vuelven a emitir SYSINFO como antes de la v0.5.15.** Se informó de que las Motorola MTP6650 y MXP600 dejaron de registrarse en estaciones v0.5.16 sin cifrado del interfaz aire. El motivo: la v0.5.15 cambió la rotación de SYSINFO de ranura a trama para que el elemento de seguridad llegue a las radios en celdas cifradas. Desde entonces toda celda, también una sin cifrado, enviaba la mitad del tiempo la variante con servicios extendidos en el canal de control (ranura 1), donde antes solo salía la definición por defecto del código de acceso A. En una celda sin cifrado ese elemento decía "clase de seguridad 1 y 2 soportadas, SCKN 0", aunque EN 300 392-7 tabla A.104 nota 1 exige que esos campos sean cero cuando la celda no tiene cifrado del interfaz aire. Las radios Motorola nunca habían visto ese elemento en una celda así. Ahora una celda sin cifrado mantiene la rotación por ranura (en el canal de control solo la definición por defecto, como hasta la v0.5.14), y las clases y el SCKN del elemento de seguridad son cero. Las celdas cifradas mantienen la rotación por trama de la v0.5.15. Gracias a DO5MF y DK7TD por probar la corrección con sus radios Motorola.

## v0.5.16 — Re-registro periódico en el servidor Brew

- **Re-registro periódico por red.** Una estación avisa a su servidor Brew de una radio una sola vez, cuando se registra; después (registro periódico, respuestas a la comprobación de presencia) no envía nada más, así que una estación tranquila puede pasar horas sin hablar de sus radios con el servidor. Algunos servidores dejan de entregar las llamadas de un grupo tras un tiempo así (visto con TetraPack). Nuevo `reregister_minutes` en el `[brew]` de cada red (o `[networks.<nombre>.brew]`): cada N minutos (10 a 600, 0 = apagado, por defecto apagado) la estación envía de nuevo cada radio registrada con REREGISTER y AFFILIATE con sus grupos, igual que cuando una radio se registra otra vez. No se da de baja nada antes, así ningún grupo queda vacío en el servidor, y espera mientras una llamada cruza el enlace. Un invitado de Multi Network va con su identidad propia. En la página Infrastructure, la tarjeta Redes tiene por cada red con servidor Brew propio (también la red única de una estación clásica) un interruptor **Re-registro** y un campo de minutos; guardar reinicia la estación. También en la página Config, en Brew → **Brew avanzado** (casilla **Re-registro** y minutos). Ver `Docs/networks.md`.
- **Documentación: la comprobación de presencia también da de baja en Brew.** Una comprobación sin respuesta envía DEAFFILIATE y DEREGISTER al servidor Brew; la radio vuelve a registrarse allí con sus grupos en cuanto responde a la siguiente comprobación (simulación nueva en las pruebas). Ver `Docs/radio-presence.md`.

## v0.5.15 — SCK clase 2: difusión del elemento de seguridad + cifrado iniciado por la red

- **La celda ahora difunde su elemento de información de seguridad.** El SYSINFO alternaba sus dos variantes según la paridad del *intervalo* (timeslot), pero el canal de control (y por tanto casi todo SYSINFO en una celda de portadora única) va siempre en el intervalo 1, de modo que la variante que lleva el Elemento de Información de Seguridad (clase de seguridad + número de SCK) no se enviaba nunca. Las radios que necesitan ese elemento para confirmar una celda de clase 2 (p. ej. Hytera) no podían activar el cifrado SCK. Ahora las variantes alternan por número de trama, así que el elemento de seguridad se difunde con regularidad en el MCCH.
- **Anunciar clase de seguridad 2 únicamente cuando no hay grupos en claro.** Una celda sin `clear_groups` deja de anunciar la clase 1 (registro en claro), de modo que una radio con clave se registra cifrada en lugar de caer a claro.
- **Cifrado SCK iniciado por la red (SwMI).** Una celda de clase 2 cifra ahora una radio con capacidad SCK que se registra sin solicitar cifrado ella misma (`cipher_control = 0`), y el D-LOCATION-UPDATE-ACCEPT incluye el elemento Security downlink (parámetros de cifrado) para que la radio conmute a cifrado. Las radios sin clave permanecen en claro.
- **Anunciar autenticación requerida** en el elemento de seguridad del SYSINFO de clase 2.
- **Mantener el modo de cifrado a través de la re-registración.** Una radio de clase 2 se re-registra periódicamente (y el propio U-LOCATION-UPDATE-DEMAND llega en claro), pero permanece cifrada todo el tiempo. `subscribers.register()` reiniciaba el indicador `encrypted` de la radio a false en cada re-registración, de modo que entre la re-registración y la activación del cifrado por el LLC-ACK del nuevo accept la radio se veía momentáneamente en claro: sus llamadas a grupos cifrados se rechazaban ("group belongs to the other encryption mode") y su enlace descendente dirigido pasaba brevemente a claro. Ahora se conserva el modo confirmado a través de la re-registración (las radios en escaneo se re-registran constantemente, lo que hacía que esto oscilara sin parar).

## v0.5.14 — Texto de inicio por ISSI y tiempo de reenvío en segundos

- **Texto por ISSI.** `[[cell_info.home_mode_display.per_issi]]` (y `sds_broadcast`) envía un texto individual a cada radio listada mientras está registrada (nada más registrarse y luego cada intervalo, con `interval_seconds` propio opcional), además de la difusión de grupo. El SDS en vivo (`POST /api/live-sds`) acepta `dest_issi` para un solo radio.
- **Tiempo de reenvío en segundos.** `[cell_info.home_mode_display]` y `[cell_info.sds_broadcast]` aceptan `interval_seconds` (tiene prioridad sobre `interval_multiframes`; 1 multitrama ≈ 1,02 s).

## v0.5.13 — Numeración de SCK desde 0 (como en el estándar y las radios)

- **El número y la versión de SCK empiezan en 0.** El número de SCK (SCKN) y la versión de SCK (SCK-VN) ahora se numeran desde 0, igual que en el estándar TETRA (EN 300 392-7, SCKN 0-31) y que los números que muestran las radios y los equipos de carga de claves. Antes la estación numeraba las SCK del 1 al 32 y enviaba el campo por el aire como SCKN − 1, es decir, con un desfase de uno respecto a una radio conforme al estándar. El número de SCK de la configuración y del dashboard ahora coincide exactamente con el que se ve en la radio.
- **Nota de migración.** Ajusta `security.aie.sckn` al número de SCK tal como lo muestra la radio (0-31). Una configuración que tuviera `sckn = 1` para alcanzar la «SCK 0» de la radio pasa a ser `sckn = 0`. El campo de versión del dashboard también parte de 0.

## v0.5.12 — Editor de configuración mejorado y comprobación previa al guardado

- **El editor ya no se come los comentarios.** Al guardar desde el configurador visual, la estación reescribía todo el `config.toml` a partir de la estructura y perdía cada comentario, línea en blanco y sección comentada. Ahora el guardado es quirúrgico (vía `toml_edit`): solo se tocan las claves que realmente cambian y se conservan los comentarios, el orden de las claves y el formato de los números. Las secciones comentadas (p. ej. una `# rx_gain = 40` dejada a propósito) sobreviven a cada guardado.
- **Editor TOML en condiciones (ya no un simple cuadro de texto).** El editor avanzado de `config.toml` pasa a tener numeración de líneas, resaltado de sintaxis —con los comentarios (y las secciones comentadas) claramente visibles en su propio color—, resalte de la línea actual y tabulación a dos espacios. Todo en el propio dashboard, sin recursos externos (funciona sin conexión en la Pi).
- **Comprobar antes de guardar (no se puede «brickear»).** Nuevo botón «Comprobar» y endpoint `POST /api/config/validate` que analiza y valida la configuración **sin escribirla**: si hay un error se marca la línea culpable en el margen y se salta a ella. El guardado y el «Aplicar y reiniciar» ya validaban en el servidor; ahora además señalan la línea del error en el editor.

## v0.5.11 — Fleet Control (fases 1 a 7)

- **Corrección: invitado de Multi Network que vuelve a su propia red.** Al pasar el MTP de la celda de otra red (como invitado, con (V)ASSI) a la celda de su propia red en la misma estación, la celda de invitado no se daba cuenta: el servidor Brew avisa «la radio se registró en otra parte» con la ISSI propia, y allí la radio solo existía con su (V)ASSI. El registro viejo quedaba unos 30 a 50 s hasta la comprobación de presencia. Ahora la celda reconoce la (V)ASSI del aviso, quita el registro enseguida y libera la (V)ASSI, que tras la migración ya no es válida (TS 100 392-2 cláusula 16.4.7.1). Además, una (V)ASSI que la comprobación de presencia ya había dado de baja ya no se da de baja una segunda vez al migrar de nuevo (esa segunda baja llegaba al servidor con la (V)ASSI sin traducir), y la reselección y el RSSI de un invitado van al servidor con su ISSI propia.
- **Última actividad: los invitados de Multi Network con su propia ISSI.** Igual que en la lista de radios registradas, la página Última actividad (y la actividad del despacho LST) muestra ahora para un invitado su propia ISSI con indicativo, la etiqueta **Invitado · red** y debajo la (V)ASSI. La entrada guarda esa identidad, así sigue legible cuando la radio ya se fue. Solo visualización.
- **Corrección: radio en silencio tras enviar un SDS durante una llamada.** Una radio que escuchaba una llamada de grupo y enviaba un SDS (p. ej. su posición LIP) en el intervalo de la llamada recibía un slot, pero sin la marca de acceso aleatorio. Por eso repetía el acceso cada pocas tramas, se rendía y dejaba la llamada: se quedaba muda hasta pulsar PTT, y mientras tanto la estación retenía al siguiente hablante de la red durante más de 1 s. Ahora la concesión lleva la marca (TS 100 392-2 cláusula 23.5.1.4.8) y el slot concedido se recibe como señalización, no como voz. La marca va solo en la concesión: si esta no llega a salir, tampoco aparece después en otro mensaje a la radio.
- **Radios registradas: los invitados de Multi Network con su propia ISSI.** Una radio de otra red (migración, TS 100 392-2 cláusula 16.4.1.1 b)) aparecía solo con su (V)ASSI de esta celda (p. ej. 15728641). Ahora la lista muestra su propia ISSI (con indicativo), debajo la (V)ASSI y una etiqueta **Invitado · red** (nombre de la red si el servidor Brew lo indica, si no MCC/MNC). Las acciones (SDS, DGNA, nombre, expulsar) siguen usando la (V)ASSI. Solo visualización; nuevo evento de telemetría `MsVisitor`, que los servidores sin él ignoran.
- **Fleet Control fase 7: carga y temperaturas para las gráficas y alarmas del servidor Brew.** Nueva capacidad `metrics-v1`. Con Fleet Control activo, la estación envía `FleetMetrics` al servidor una vez por minuto: carga de CPU (media del minuto), temperatura de CPU, por celda el estado RF (con el texto del error si el SDR no se pudo abrir) y la temperatura del SDR, las radios registradas, las llamadas en curso y el estado de salud de la pila. No se envía nada a un servidor sin Fleet Control ni con el interruptor apagado. El servidor dibuja con ello gráficas de 24 h y genera alarmas (inalcanzable, error de SDR, temperatura, carga de CPU). Necesita un brew-server con la fase 7 de Fleet Control. Ver `Docs/fleet-control.md`.
- **Desactivar Fleet Control o el comando / control remoto pide confirmación dos veces.** Al apagar el interruptor de Fleet Control, o el de comando / control remoto al final de la página Infrastructure, primero se explica qué deja de funcionar (la gestión remota de esta estación base) con **Lo he entendido** a la izquierda y **Cancelar** en rojo a la derecha; después se pregunta otra vez si estás seguro, con **Cancelar** a la izquierda y **Sí, desactivar** en rojo a la derecha, así un doble toque no pasa. Si se desactiva el canal de control con Fleet Control activo, primero se desactiva Fleet Control, para que el servidor Brew lo anote para sus administradores y gestores de flota.
- **Página Infrastructure: MCC/MNC siempre visible.** La tabla de vecinas (también las celdas propias de la estación) y la lista de celdas del servidor Brew muestran el MCC/MNC de cada entrada. En el editor de vecinas, un MCC/MNC vacío muestra la red de la propia celda como indicación («Vacío = la red de esta celda»). Con Multi Network, una entrada sin MCC/MNC se compara ahora con la red de la propia celda, no con la principal de la estación (TS 100 392-2 tabla 18.64).
- **Página Infrastructure: avisos de la celda del servidor bajo su id.** En el móvil un aviso como «no incluye esta celda» quedaba al lado del id de la celda y lo partía en trozos; ahora va en su propia línea bajo el id.
- **Página Sistema: idioma predeterminado.** Nuevo desplegable **Idioma predeterminado** (`[dashboard] language`, el mismo ajuste que en el asistente de configuración). Un cambio reinicia el servicio, así todas las páginas y el inicio de sesión vuelven en ese idioma.
- **Corrección: cambio de grupo durante una llamada de red.** Si la última radio de un grupo cambiaba a otro grupo mientras alguien hablaba por la red, la voz de esa llamada seguía llegando al mismo intervalo de tiempo y se mezclaba con la siguiente llamada que lo usaba (audio entrecortado, bloques de voz descartados). Ahora la llamada queda en espera correctamente en Brew y vuelve con entrada tardía al volver al grupo.
- **Fleet Control: más ajustes desde el servidor.** `SetConfig` acepta ahora `raw`: valores sueltos del archivo de configuración (el editor de configuración del servidor envía solo lo que cambió). Nunca contraseñas ni claves. `GetConfig` incluye además el Home Mode Display y la lista blanca de ISSI.
- **Página Infrastructure: mensaje al guardar.** Guardar ahora muestra «Infrastructure settings saved» en lugar de «roaming settings saved».
- **Fleet Control fases 4/5: leer y cambiar la configuración desde el servidor Brew.** Nueva capacidad `config-v1`. El servidor lee la configuración (`GetConfig`: la vista de la página Infrastructure sin contraseñas ni claves, más el archivo con contraseñas y claves sustituidas por `***`) y la cambia (`SetConfig`): la estación comprueba toda la configuración igual que su propio dashboard, guarda la anterior (`<config>.fleet-backup`), escribe la nueva y reinicia el servicio. Después tiene 180 s para volver al servidor (y a Brew); si el servidor no confirma (`ConfirmConfig`), restaura sola la configuración anterior y reinicia otra vez, así un cambio que la deja sin conexión se deshace solo. `RollbackConfig` la restaura antes. El `StationHello` incluye ahora el hash de la configuración y si hay un cambio esperando confirmación, para que el servidor vea cuando se ha editado la configuración en la estación. Las claves (`[security]`) nunca se leen ni se escriben. Requiere brew-server con Fleet Control fases 4/5. Ver `Docs/fleet-control.md`.
- **Corrección: página Llamadas en el móvil.** En pantallas estrechas las columnas ID y Hablante estaban ocultas, así que solo se veía quien inició la llamada y su ISSI se quedaba fija. Ahora la columna Hablante se ve también en el móvil y se actualiza en vivo con quien habla.
- **Fleet Control: aviso de mantenimiento también sin iniciar sesión.** El aviso rojo de mantenimiento / actualización en vivo ahora también aparece en la página de inicio de sesión y en la vista pública. Lee el nuevo `GET /api/fleet-banner`, abierto sin sesión, que solo devuelve el estado del aviso (sin direcciones ni usuarios).
- **Fleet Control: aviso de mantenimiento y reinicio desde el servidor.** Mientras el servidor Brew tiene activado el mantenimiento de la estación, todas las páginas del panel muestran un aviso rojo bajo la barra superior ("Mantenimiento: se está trabajando en esta estación base", quién, minutos restantes, nota). Se apaga solo (como máximo 240 min) y vuelve tras un reinicio mientras el tiempo del servidor no haya terminado. Una actualización en vivo muestra su propio aviso rojo "Actualización en vivo: ¡RF activa!" con el porcentaje de compilación. El servidor también puede reiniciar el servicio o el sistema completo (`RestartStation`, rechazado durante una actualización y, salvo que se pida, durante una llamada); no hace falta ninguna regla sudo porque el servicio se ejecuta como root. Nuevas capacidades `maintenance-v1` y `restart-v1`. Ver `Docs/fleet-control.md`.
- **Fleet Control: actualización en vivo con la radio encendida.** El servidor Brew puede enviar un repositorio y una rama con `StartUpdate` (su "Updates repo"). La estación ejecuta entonces su instalador con ellos, igual que `install-bost.sh` en un terminal, y sigue en el aire mientras compila; solo el reinicio del servicio al final la saca del aire (unos 2 min). Después, la actualización normal de la estación sigue ese repositorio y esa rama. El instalador se ejecuta sin la prioridad de tiempo real del servicio (como tarea normal de menor prioridad), así la compilación no quita tiempo a la radio. Solo se aceptan repositorios `https://` simples y nombres de rama simples. Nueva capacidad `update-live-v1`. Ver `Docs/fleet-control.md`.
- **Fleet Control fase 2: actualización remota desde el servidor Brew.** Con Fleet Control activado, el servidor puede pedir a la estación que compruebe actualizaciones (`CheckUpdate` → `UpdateInfo`: versión actual, la más nueva con su commit (p. ej. `v0.5.9-d081d936`), origen OTA) e iniciar la misma actualización OTA que el botón Actualizar del dashboard (`StartUpdate` → `CommandResult`: `ok`, `busy`, `call_active`, `unavailable`). La estación se actualiza siempre desde su propio origen OTA (`[dashboard] ota_channel`, por defecto el repositorio y la rama instalados); el servidor nunca elige repositorio ni rama. Durante cada actualización (la inicie el servidor o el dashboard) la estación informa `UpdateProgress`: etapa (descarga, compilación, instalación, reinicio, al día, error), porcentaje estimado y última línea del registro. Sin `during_call`, la estación rechaza el inicio mientras hay una llamada en curso. Nueva capacidad `update-v1` (solo con el dashboard en marcha). Nota: la estación está fuera del aire durante la compilación (10–20 min en una Raspberry Pi), como en cada OTA. Requiere brew-server con Fleet Control fase 2. Ver `Docs/fleet-control.md`.
- **Fleet Control: gestión de la estación por el servidor Brew (SwMI).** Página Infrastructure, bajo la línea Brew (siempre visible, encima de las Funciones del servidor): nuevo interruptor **Fleet Control** («Gestión de la estación por la SwMI (BrewServer)»), apagado por defecto y visible solo si el servidor lo ofrece (`fleet-control` en `X-Brew-Extensions`). Funciona al momento, sin reiniciar (`[command] fleet_control = true`). Si el canal de control (`[command]`) no está configurado, un diálogo pide servidor (nunca prerrellenado), puerto (443), TLS (activado), usuario (el login de Brew) y contraseña (vacía = la de Brew); al guardar se reinicia el servicio. En la barra lateral, bajo Health, el campo **Fleet Control** muestra verde activado / rojo desactivado. Con el servidor que lo ofrece, la estación envía `StationHello` (versión, login de Brew, red, interruptor, capacidades) al conectar y en cada cambio del interruptor, y responde a los comandos de Fleet Control con `CommandResult`. Los comandos existentes (kick, DGNA, SDS, reinicio) no dependen del interruptor. Requiere brew-server con Fleet Control. Ver `Docs/fleet-control.md`.

## v0.5.10 — Ajustes del tiempo APRS en el dashboard

- **Ajustes del tiempo APRS en el dashboard (PR #59, dn9cho).** En la página Config, la tarjeta «Servicio WX» pasa a ser «Servicio WX (METAR / APRS)», con tres secciones —General, METAR y APRS— y un solo botón Guardar. Ahí se configura el servicio meteorológico APRS (`APRSWX`, `[aprs_wx]`): clave API de aprs.fi (nunca se muestra; dejar el campo vacío conserva la guardada), estación por defecto, antigüedad a partir de la cual un informe se marca como antiguo y el envío periódico (estación, destino, grupo o ISSI, intervalo). Los cambios se aplican al momento, sin reiniciar, y se escriben en `config.toml`. Las pistas de estos campos y del intervalo METAR pasan a botones de ayuda «?». METAR funciona como antes.
- **Sin aviso de fallo de entrega a servicios locales (PR #60, dn9cho).** Cuando un SDS no puede entregarse dentro del plazo de espera (destino en una llamada o dormido en ahorro de energía), la estación solo avisa del fallo al remitente si este es un terminal registrado en la celda. Antes, un METAR o un informe APRSWX periódico no entregado también generaba un aviso de fallo al ISSI del servicio WX, que no es un terminal, y la estación lo retransmitía a nadie hasta `exhausted retransmissions`.
- **Ejemplos neutros para APRSWX (PR #60, dn9cho).** Ejemplos, textos de ayuda y tests usan `N0CALL-13` en lugar del indicativo de una estación meteorológica real, también en la pista que recibe un terminal al enviar `APRSWX` sin indicativo si no hay estación por defecto.

## v0.5.9 — Calibración de ganancia RX y listas de escaneo largas

- **Corrección: listas de escaneo de más de 12 grupos (PR #55, dn9cho).** Con radios que escanean 13 grupos o más, la estación repetía `SwMI group report requested` sin fin y los grupos a partir del 13.º nunca se adjuntaban ni se afiliaban en Brew. Si la radio responde al informe de nuevo con toda la lista (visto con Motorola MTP850 y MTM5400), ahora la estación adjunta ella misma los grupos restantes (`SwMI D-ATTACH remainder`, ETSI §16.8.1). Las listas de hasta 12 grupos no se ven afectadas.
- **Calibrar las ganancias RX del SDR desde el dashboard (SX1255/SX1257, LimeSDR).** La página RF tiene una nueva tarjeta «Calibración de ganancia RX». «Calibrar ganancias» barre el LNA y el PGA en el SDR en vivo, mide el ruido de fondo y la fuga de LO/DC en cada paso (o, en modo «Uplink en vivo», la mejor relación señal/ruido de una radio transmitiendo en la célula) y recomienda el LNA/PGA que dan más sensibilidad sin saturar el ADC ni dejar que la fuga domine; después restaura tus ganancias actuales. Un clic las rellena y las aplica al SDR en marcha para que oigas el efecto (guárdalas en Setup para conservarlas tras reiniciar). Pensado para placas sin PA, donde el reparto de ganancia es por lo demás pura conjetura. El barrido perturba brevemente la recepción, y una confirmación avisa antes de empezar. Una sección «Avanzado» lee y escribe registros SX1255 directos para quien trabaje con la hoja de datos. En estaciones multicélula la calibración actúa sobre el SDR primario.
- **Separaciones duplex de 9 MHz y otras no estándar, más fáciles de configurar.** La tarjeta RF / Frecuencias muestra ahora una pista sobre separaciones no estándar, y «Auto RX + carrier» indica la separación usada (p. ej. «duplex 9 MHz»), para que se vea cuando la separación supuesta es incorrecta. Define la separación en RF avanzada → Duplex personalizado (MHz) —introduce `9` para una separación de 9 MHz— y «Auto RX + carrier» rellena el uplink RX como TX menos esa separación. El campo de duplex personalizado ya existía; esto lo hace visible y muestra el valor que supuso.

## v0.5.8 — Origen de actualización, idioma de la estación, calculadora de carrier, telemetría

- **Actualizar desde cualquier repositorio y rama, sin SSH.** El diálogo de actualización tiene un selector de origen: **repositorio instalado** (por defecto: el repositorio y la rama desde los que se instaló la estación, así que una estación instalada desde un fork sigue actualizándose desde él), **Estable** / **Beta** (repositorio original `main` / `beta`) y **URL git propia** con rama. Así los desarrolladores pueden volver a `main` o probar otra rama o fork desde el dashboard: elegir origen, comprobar, actualizar. Antes, cada actualización devolvía la estación al repositorio original. Los commits hechos en la propia estación siguen bloqueando la actualización; los que están en GitHub (p. ej. la rama del fork que se deja) no. Configuración: `[dashboard] ota_channel = "installed" | "stable" | "beta" | "custom"` con `ota_repo` / `ota_branch`; el instalador escribe `installed`. La comprobación de versión y las notas vienen del repositorio elegido (solo GitHub; otro servidor se actualiza igual con git). Solo URL https y ssh; las credenciales nunca se muestran ni se registran. Para volver a un `main` anterior a 0.5.8: elegir Estable, que las versiones antiguas también entienden.
- **Página System: «Instalado desde».** Bajo la versión: repositorio y rama desde los que funciona la estación, con enlace a GitHub.
- **Idioma de la estación.** El asistente de configuración empieza con el idioma (`[dashboard] language`), que pasa a ser el predeterminado del dashboard y de la página de acceso (antes: español); un navegador que eligió su propio idioma lo conserva. Las estaciones configuradas antes lo preguntan una vez tras el siguiente acceso. Las notas de actualización salen de `CHANGELOG_en.md` para cualquier idioma salvo el español.
- **Paso de frecuencia en el asistente de configuración.** Tras elegir el SDR, el asistente pide la frecuencia TX, la separación dúplex (id de tabla o MHz propios), la operación inversa, el colour code y la location area, muestra RX, banda, offset y main carrier en directo y lo guarda sin activar RF.
- **Calculadora de carrier.** En Config → RF, al salir de «Downlink TX (MHz)» se fijan banda, offset, main carrier y RX, para cualquier banda, con operación inversa y el dúplex del id de tabla (antes: siempre 5 MHz y solo banda 4). Radios con tabla de dúplex propia: id de dúplex como en la radio (p. ej. `duplex_spacing = 7`) más «Dúplex personalizado (MHz)» (7,6 se completa a 7.600000 y se guarda como `custom_duplex_spacing = 7600000`).
- **Telemetría y control remoto desde el dashboard (PR #51).** Página Infrastructure: secciones desplegables «Telemetry» (`[telemetry]`) y «Remote command / control» (`[command]`) con interruptor (`enabled = false` conserva los valores sin conectar). La contraseña nunca se muestra; vacía conserva la guardada. Una telemetría nueva empieza en Viena (48.2083, 16.3725).
- **Controlador Pluto / Pluto+.** «Instalar Pluto» en el asistente y en la página Setup compila SoapyPlutoSDR con flujo con marcas de tiempo (pgreenland, `sdr_gadget_timestamping`) cuando se pide; el instalador no lo compila. Device `driver=plutosdr`.
- **El dashboard muestra a los hablantes de la red en el hangtime.** Una radio que toma la palabra por Brew durante el hangtime de una llamada de grupo en curso actualiza ahora el hablante actual y «Last Heard» (antes: solo tras una llamada nueva). El aire no cambia. Probado en simulación con 10 estaciones y 30 radios.
- **Instalador.** Registra `/opt/bost-flowstation` como `safe.directory` de git (se acabó el «dubious ownership» al actualizar) y muestra el repositorio que realmente instaló.
- **Página Infrastructure: márgenes.** Los textos de ayuda, contadores y controles de las tarjetas ya no quedan pegados al borde; las tablas siguen de borde a borde.

## v0.5.7 — SC3 (TEA2), grupos gestionados y correcciones

- **Los paquetes de estación rechazan rutas absolutas y de salto de directorio al extraerse.** Al importar un paquete de estación, una entrada con ruta absoluta o con `..` ya no puede escribir fuera del directorio de destino (zip-slip). Corrección aportada por David Harcus, MM7FDM (PR #46).
- **Cifrado de la interfaz aérea clase de seguridad 3 (SC3) con TEA2 — experimental.** Cada radio obtiene su propia DCK en la autenticación con su K (también autenticación mutua); los enlaces descendentes de grupo usan una CCK común que la estación entrega sellada (TA31/TA32) en la aceptación de la actualización de ubicación y a petición (`U-OTAR CCK DEMAND`), y la página Security puede reenviarla (`D-OTAR CCK PROVIDE`). La nueva DCK entra en vigor con el acuse del resultado de la autenticación (EN 300 392-7, cláusula 4.5.5.1). Sin GCK, sin rotación de CCK, sin recuperación de DCK. Se configura en Security → Air-interface encryption (`[security.aie] class = 3`, `cck`, `cck_id`, autenticación `required`). Registro, autenticación mutua y voz cifrada confirmados en el aire con dos radios Motorola. Parche aportado (revisión sc3-r4, visible en la página Security); integrado en la versión 0.5.7.
- **SC3 funciona en una sola celda.** La DCK se queda en la celda que autenticó la radio, así que una configuración con clase de seguridad 3 y `[[cells]]` se rechaza (al arrancar y al guardar en el dashboard), y una estación SC3 no ofrece roaming entre estaciones al servidor Brew (sin `handover-v1`, `ms-moved`, `call-restore`); una radio que llega de otra estación se registra y autentica de nuevo. SC1 y SC2 no cambian.
- **Voz TEA2/TEA3: secuencia de cifrado sobre los bits del códec en su orden original.** El cifrado de voz aplicaba la secuencia después de reordenar los bits por clase de sensibilidad, lo que podía dejar la voz ininteligible pese a claves correctas (SC2 y SC3). Ahora el cifrado va antes de la codificación de canal, como lo sitúa EN 300 392-7, cláusula 6.1.
- **La lista de grupos con «detach all» solo cambia lo que cambia.** Cuando una radio sustituye toda su lista de grupos (tabla 16.49), solo se desafilian los grupos que faltan en la nueva lista; los que conserva no se tocan y ya no se envían por segunda vez al servidor Brew. Para la radio el resultado es el mismo que antes (una llamada en curso en un grupo conservado continúa), con menos tráfico.
- **Un informe de grupos no registra una radio.** Un informe de grupos de una radio que la estación no conoce (p. ej. tras un timeout) la registraba sin avisar y sin autenticación. Ahora se responde con un rechazo y un `D-LOCATION UPDATE COMMAND`, para que la radio se registre correctamente (con autenticación si está configurada).
- **Una celda de otra red se anuncia con su propio número.** Una celda adicional que sirve otra red (con su propia conexión Brew) se anunciaba al servidor como celda 0, así que el dashboard del brew-server mostraba dos veces `<login>/0 primary`. Ahora se anuncia con su número (`<login>/1`) y las radios registradas en ella aparecen en esa celda (`ms-cell`).
- **Las radios en una celda de otra red aparecen en esa celda.** Una radio registrada en una celda así (p. ej. por migración) se comunicaba al servidor sin su celda, porque la conexión Brew de la celda nombraba la celda primaria, que no sirve. El registro de roaming mostraba entonces solo `<login>` en lugar de `<login>/1`. Ahora la celda se comunica a sí misma.
- **Una red con servidor propio desde el dashboard.** En la tarjeta «Networks» de la página Infrastructure, «Acceso» ofrece ahora también «servidor propio»: host, puerto, TLS, usuario y contraseña del servidor Brew de esa red (`[networks.<nombre>.brew]`, de la PR #38). Antes solo se podía añadir en la configuración.
- **Grupos gestionados (managed talkgroups) del servidor Brew.** Si el brew-server tiene activado «Managed talkgroups» (`"managed":true` y `"listed"` en `SERVICE_GROUP_ACCESS`), la estación solo deja asociar y usar los grupos listados: la asociación a cualquier otro grupo se rechaza en el aire igual que arriba («Temporary 1 detachment», cláusula 16.8.2, tabla 16.52) y una llamada a él no se establece (`D-RELEASE`). Las llamadas de emergencia pasan; los grupos de la red de origen de un visitante ((V)GSSI) no se comprueban. Si el servidor lo desactiva, todos los grupos vuelven a estar abiertos al momento; con otros servidores no cambia nada.
- **Corrección: las llamadas de grupo largas ya no se cortan a los seis minutos (PR #44, dn9cho) — probado al aire el 2026-11-08.** Una llamada de grupo de más de seis minutos (p. ej. un grupo de conversación Brew concurrido que el hangtime mantiene abierto, con `call_timeout_secs = 0` o de 541 en adelante (ocho minutos o más), como sugiere la configuración de ejemplo para QSO largos) la cerraba la caducidad de seguridad del gestor de circuitos sin avisar a Brew: las radios quedaban en silencio en mitad del pase y el log mostraba `need to send CLOSE for call id …` y después `rx_tmd_prim: dropping DL voice on inactive circuit` hasta que el siguiente pase establecía una llamada nueva. La caducidad omite ahora los circuitos cuya llamada sigue activa. Los circuitos huérfanos se siguen cerrando a los seis minutos; las llamadas activas terminan como antes por el hangtime, `call_timeout_secs` y los temporizadores de inactividad. Con los valores por defecto (`call_timeout_secs = 120`) no cambia nada. Al aire: TG91, unas 3 h, 89 llamadas, 7 de más de seis minutos (la más larga de 19 min), ningún corte.

## v0.5.6 — Roaming entre estaciones (Brew) y entre celdas


- **Incluye cuatro pull requests de `beta`**: de David Harcus, MM7FDM, #37 (corrección del canal de datos por paquetes del WAP), #38 (varias redes en una estación, la base de Multi Network) y #39 (herramienta de precisión de portadora); de dn9cho, #40 (estaciones meteorológicas APRS por SDS). Sus entradas están en «Not yet released» más abajo.
- **Multi Network y perfil Brew en la página Config.** Con Multi Network apagado, aplicar un perfil Brew ya no falla con «this station uses [networks]»: un `[networks]` que solo contiene la red principal vuelve a `[net_info]` + `[brew]`. Apagar Multi Network quita `[networks]` de la configuración (la red principal vuelve a `[net_info]` + `[brew]`, las demás redes y las asignaciones de celdas se borran, tras confirmar; se guarda copia), y solo se pueden añadir redes con Multi Network activo. Con varias redes el campo «Core Net (Brew)» aparece gris como «Multi Network activo»: los enlaces al núcleo se editan en Infrastructure, y aplicar o guardar cambios de celda (hangtime, etc.) funciona sin tocarlos.
- **Roaming entre estaciones.** Las estaciones de un mismo servidor Brew mantienen las llamadas de una radio cuando pasa de una a otra, por reselección anunciada o no anunciada (ETSI TS 100 392-2 cláusula 18.3.4). Llamadas de grupo: quien escucha sigue oyendo el turno en la nueva estación y quien habla con PTT conserva la palabra. Las llamadas individuales símplex y dúplex con un participante Brew se restauran en la nueva estación (`U-CALL RESTORE`); una llamada que la radio abandona termina al momento para el otro. La estación anterior suelta la radio sin dejar registros ni canales. Una reselección anunciada con celda destino prepara la estación destino a través del servidor (tipos 1/2); el tipo 3 funciona sin él. Requiere [brew-server](https://github.com/ysamouhos/brew-server) 1.20.0 o posterior (cuando se publique) con las extensiones `bts-info`, `handover-v1`, `ms-cell`, `ms-moved`, `ms-reselection`, `call-restore`; otros servidores (TetraPack, Brandmeister) funcionan como antes. Probado en el aire con dos estaciones y un Motorola MTP6650. Ver «Roaming between stations over Brew» en el README.
- **Multi-celda: las llamadas siguen a la radio entre las celdas de una estación — provisional, aún sin validar al aire.** Las llamadas individuales símplex y dúplex con un participante Brew (p. ej. la app TetraLink) continúan cuando la radio cambia de celda en plena llamada: la nueva celda contesta al `U-CALL RESTORE` de la radio con `D-CALL RESTORE` (cláusula 14.5.1.2.4). Una llamada que la radio abandona, o que no se restaura en 8 s, termina al momento para el otro. Se basa en el traspaso de llamadas individuales de David Harcus, MM7FDM (PR #30).
- **Multi-celda: las celdas pueden compartir área de localización — provisional, aún sin validar al aire.** Dentro de su área de localización una radio cambia de celda sin registrarse (cláusula 18.3.4.7.1a). Las llamadas de grupo se emiten en todas las celdas del área, las comprobaciones de presencia se envían en todas para que la radio se registre donde está, y una llamada individual desde la red se ofrece en todas las celdas del área; la celda donde contesta la radio se la queda.
- **Las estaciones multi-celda hacen roaming con otras estaciones — provisional, aún sin validar al aire.** Una estación con varias celdas participa en el roaming entre estaciones como una de una sola celda: registros reenviados, reselección anunciada y llamadas restauradas se atienden en la celda correcta. Entre estaciones ofrece solo el tipo 2.
- **Nueva página del panel «Infraestructura».** (En versiones anteriores se llamaba «Roaming / Celdas»; icono del menú: una torre de radio unida a sus celdas y redes.) Una página para las celdas de esta estación y todo lo que decide cuándo y cómo pasan las radios: la tarjeta Celdas (añadir y quitar celdas, escaneo de SDR, área de localización opcional para una celda nueva; traída aquí desde Inicio), id. global (`<login Brew>/<celda>`) y área de localización de cada celda, la lista de vecinas de cada celda (las celdas adicionales pueden tener ahora la suya, `[[cells.cell_info.neighbor_cells_ca]]`), nivel mínimo de acceso, migración, comprobación de presencia y los valores de reselección en «Reselección de celda (avanzado)». Cada campo tiene ayuda «?» con la cláusula ETSI. Al guardar se comprueba la configuración, se escribe `config.toml` (el archivo anterior queda en `config.toml.cells.bak`) y la estación se reinicia.
- **Celdas del servidor Brew en la página Infraestructura.** Con un brew-server que ofrece `cell-directory` la página lista las celdas de las demás estaciones (en línea o no, portadora y frecuencia de bajada, LA, nivel de acceso, potencia MS, sus vecinas), avisa cuando una vecina no coincide con lo que emite esa celda o cuando esa celda no anuncia esta, y «Añadir como vecina» rellena una entrada desde la lista.
- **Nivel mínimo de acceso configurable.** `[cell_info] rxlev_access_min_dbm` (−125 a −50 dBm en pasos de 5 dB, por defecto −110), enviado como RXLEV_ACCESS_MIN en SYSINFO (tabla 21.65); las celdas adicionales pueden cambiarlo. También en Config → Cell/RF → Advanced RF, junto a una casilla «Migración soportada» para `[cell_info] migration` (activa el bit en SYSINFO; aceptar radios de otras redes además necesita `[multi_network]`, ver abajo). Guía: `Docs/cell-reselection.md`.
- **Las llamadas entre dos radios sobreviven a un cambio de estación — provisional, aún sin validar al aire.** Una llamada símplex o dúplex entre dos radios de una misma estación continúa cuando una de ellas pasa a otra estación y la restaura allí: la estación entrega la llamada a la red (la pata que se queda conserva su canal, la radio que se queda no nota nada; quien hablaba y se fue pierde la palabra en el corte, cláusula 14.5.1.2.4). Una llamada dúplex cuya pata queda en silencio se mantiene 8 s para ello. Al revés, una radio que llega a la estación de su interlocutor vuelve a unirse allí en una llamada local. Requiere brew-server con la extensión `call-handoff`; entre estaciones, por ahora solo estaciones de una celda.
- **Multi-celda: las llamadas entre dos radios sobreviven a un cambio de celda — provisional, aún sin validar al aire.** Una llamada símplex o dúplex entre dos radios de la estación continúa cuando una de ellas cambia de celda y la restaura (`U-CALL RESTORE`, cláusula 14.5.1.2.4), anunciado o no: con las dos radios en una celda, esa celda entrega la llamada a la nueva a través del conmutador del sitio; con las radios en dos celdas, la celda a la que llega una radio vuelve a unir las dos patas en una llamada local. La radio que se queda no nota nada, quien hablaba y se fue pierde la palabra en el corte, y una pata que queda en silencio se mantiene 8 s para la restauración. Dentro de la estación no hace falta ninguna extensión del servidor. Completa los puntos abiertos del traspaso de llamadas individuales del PR #30 (David Harcus, MM7FDM) para dos radios de la estación; un cambio anunciado a la propia celda del interlocutor ya no pre-provisiona allí una segunda pata.
- **Quién puede usar un grupo, desde el servidor Brew.** Con un brew-server que ofrece `group-access` la estación recibe en vivo las listas de cada grupo (permitir solo estas ISSI o bloquear estas, y «solo escucha» para todas o para algunas) y las aplica ella misma: una radio que el grupo no admite no puede asociarse (`D-LOCATION UPDATE ACCEPT` / `D-ATTACH/DETACH GROUP IDENTITY ACK` la listan como rechazada, «Temporary 1 detachment», cláusula 16.8.2, tabla 16.52), una radio de solo escucha entra en la llamada sin poder pedir la palabra (cláusula 14.8.43) y su PTT recibe «transmission not granted» (cláusula 14.5.2.2.1 b)); quien no puede hablar tampoco puede iniciar una llamada al grupo (`D-RELEASE`, causa «Requested service not available»). Las llamadas de emergencia pasan siempre. Los cambios en el panel del servidor llegan sin reiniciar; con otros servidores todo sigue igual.
- **Funciones del servidor en la página Infraestructura.** Las extensiones Brew que ofrece el servidor aparecen en un desplegable «Funciones del servidor» agrupadas (Roaming, Llamadas, Celdas, Grupos) como etiquetas con un punto verde (ofrecida) o rojo (no ofrecida), ahora también `call-handoff`, `group-access`, `issi-check` y `mni` (Multi Network: el servidor comprueba si una ISSI ya está en uso en la otra red antes de registrarla allí, y acepta radios de otras redes en estaciones con `mni`).
- **Migración: radios de otras redes en esta estación (Multi Network, paso 6) — probada al aire el 2026-11-07** (una radio de la red del servidor Brew y una radio de una tercera red enlazada al servidor, 901/1234; una red que el servidor no tiene activa se rechaza). Con `[multi_network] enabled = true` y la red de origen (MCC/MNC) en `networks`, una radio de otra red se registra aquí por migración (ETSI TS 100 392-2 cláusula 16.4.1.1 caso b)): la estación contesta al primer `U-LOCATION UPDATE DEMAND` en la USSI con `D-LOCATION UPDATE PROCEEDING` y una (V)ASSI, y la radio se registra con ella (cláusula 16.4.7). Sus grupos de origen reciben una (V)GSSI (cláusula 16.8.7); los grupos de esta red mantienen su GSSI. Una radio de esta red que vuelve de otra (caso c)) se registra como siempre. Con un brew-server que ofrece `mni` el visitante llega a su red de origen a través del servidor (ver abajo); con otros servidores se queda dentro de la estación. `[multi_network]` activo enciende la migración por sí solo (detalles de servicio en SYSINFO, cláusula 18.5.2.1), así que ya no hace falta `migration = true` en `[cell_info]`; el panel muestra la casilla marcada y en gris. Si una radio migra o se registra con identidad propia en la otra red lo decide un ajuste de la radio por red (ITSI attach o migración); `Docs/networks.md` explica ambos, con los campos del CPS de Motorola. Las reglas de acceso a grupos del servidor Brew comprueban una radio migrada de la red propia del servidor con su ISSI de origen, no con su (V)ASSI (en la prueba de campo se rechazaba su grupo). Una llamada individual o SDS del servidor para esa radio le llega en su (V)ASSI. Una llamada de grupo de la red del servidor solo llega a esa celda en la (V)GSSI de una radio migrada; lo demás se descarta, así que un grupo de la red del servidor nunca se emite como grupo de la red de la celda (en la prueba de campo se oía la llamada de una app en una celda de TetraPack). La migración desde la red propia del servidor a una celda `via` funciona al aire (Motorola MTP6650, 2026-11-07). Así la celda decide en qué red habla una radio («cambio de red por celda»): con origen 901/9999, en una celda 9999 es radio del servidor Brew, en una celda 901/1638 migra y sus grupos son de TetraPack / Brandmeister (tabla en `Docs/networks.md`). Nota: una radio migrada que marca solo un SSI llama a un abonado de la red de la celda (TS 100 392-2 cláusula 14.2.3: el SwMI toma el MNI que falta como el de la red actual); para llegar a su red de origen tiene que enviar la TSI completa. Un interruptor junto al título «Redes» de la página Infraestructura activa y apaga `[multi_network]` (guarda y reinicia). La tarjeta lista las redes que anuncia el servidor Brew (`SERVICE_NETWORKS`: su red propia y las que enlaza, activas o caídas) con «Añadir red», que toma su MCC/MNC para que todas las estaciones usen los mismos valores, y marca cada red: enrutable, enlace caído o «No es posible enrutar con este MCC/MNC» cuando el servidor no llega a ella (también si el MCC/MNC de la red principal no es el propio del servidor). Sin la sección todo sigue igual: «Migration not supported», ahora contestado en la USSI como pide la cláusula.
- **Multi Network con el servidor Brew (paso 7) — probado al aire el 2026-11-07** (una radio de TetraPack en una celda `via` como visitante: llamadas de grupo, llamadas dúplex y SDS entre las redes). Con un brew-server que ofrece `mni` la estación lo anuncia en `SERVICE_BTS_INFO` y presenta a cada visitante con su identidad de origen: `SERVICE_MS_NETWORK` antes de su registro, su ISSI de origen en lugar de la (V)ASSI y el GSSI de origen en lugar de la (V)GSSI (`SERVICE_GROUP_NETWORK` para grupos de esta red, `SERVICE_CALL_NETWORK` antes de una llamada de grupo de su red, y con la red del que llama en llamadas individuales y SDS). En sentido contrario, una llamada de grupo de la red de origen llega en la (V)GSSI y una llamada o SDS al visitante en su (V)ASSI. La migración solo se acepta desde redes que el servidor alcanza y tiene activas (`SERVICE_NETWORKS`), y además en `networks` si está configurado. Las (V)ASSI y (V)GSSI nunca salen al aire de la red.
- **Multi Network: se rechaza una radio de otra red que no migra — provisional, aún sin validar al aire.** Con `[multi_network]` activo, una actualización de localización que nombra otra red (extensión de dirección) pero no es migratoria, p. ej. una radio que contesta a un `D-LOCATION UPDATE COMMAND` después de cambiar su red de origen, ya no se registra con su ISSI sola (esa es una radio de esta red) sino que se rechaza con «ITSI/ATSI unknown» (ETSI TS 100 392-2 cláusulas 16.4.1.1 b), 16.4.7.3), para que la radio se registre de nuevo como migratoria. Sin la sección todo sigue igual.
- **Entrada tardía más rápida tras cambiar de estación.** La estación repite el D-SETUP de la llamada de grupo en cuanto la radio que llega vuelve a asociar sus grupos: un hueco de 1 s en lugar de 5 s.
- **Corrección AIE: la señalización en el canal de tráfico (FACCH/STCH) va cifrada.** Con cifrado de interfaz aire (TEA) los mensajes que la estación roba de un canal de voz (p. ej. D-TX GRANTED / D-TX CEASED) salían en claro a radios registradas cifradas. Ahora el MAC-RESOURCE se guarda tal cual hasta el slot real de transmisión y se cifra allí con la secuencia de clave de ese slot: ESI en lugar del ISSI, KSS(0 a 123) en el primer medio slot y KSS(216 a 339) para un MAC-END en el segundo (ETSI EN 300 392-7 cláusulas 6.4.1 tabla 6.4 y 6.7.1.2). Si no se puede cifrar, no se envía en claro. Gracias a [lx-dxb](https://github.com/lx-dxb) ([BOST-FlowStation-FACCH-STCH-Air-Interface-Encryption-Fix](https://github.com/lx-dxb/BOST-FlowStation-FACCH-STCH-Air-Interface-Encryption-Fix)).
- **Corrección OTAR: «Enviar SCK» desde el panel llega a la radio.** El panel mandaba todas sus órdenes a CMCE, que descartaba el OTAR de SCK («ignoring unsupported control command OtarSck»), así que la clave nunca salía por el aire. Ahora el panel tiene su propio canal a MM: OTAR de SCK y DGNA van directamente a MM, y el panel y el control remoto usan la misma asignación orden → entidad. Gracias a [lx-dxb](https://github.com/lx-dxb) ([bost-flowstation-otar-fixed](https://github.com/lx-dxb/bost-flowstation-otar-fixed)).
- **Sin «-modified» tras instalar.** El instalador ya no modifica un archivo del repositorio, así que las versiones ya no muestran `-modified` y los cambios locales ya no bloquean una actualización.

## v0.5.5 — Wizard µCell (SoapyMuCell)

- **Wizard µCell.** El asistente y la página Setup pueden instalar el driver `mucell` (placa µCell BB, distinto de SXceiver): clona/compila SoapyMuCell y el overlay `mu-cell-bb_raspberrypi`, escribe `device = "driver=mucell"`, y si el helper imprime `NEED_REBOOT` ofrece reiniciar la Pi (`POST /api/system/reboot`) para cargar el device-tree. Familias de ganancia/antenas alineadas con SXceiver; validación `SoapyDriverFamily::MuCell`.

## Pendiente de publicar — Autenticación, TEA2/TEA3, OTAR, E2EE de SDS, WAP y USRP B210

- **Estaciones meteorológicas APRS por SDS (`APRSWX`, PR #40, dn9cho).** El servicio WX acepta un tercer comando: un SDS `APRSWX <INDICATIVO>` (p. ej. `APRSWX N0CALL-13`) al ISSI del servicio devuelve el último informe de esa estación meteorológica APRS/CWOP desde aprs.fi (temperatura, humedad, presión, viento y racha, lluvia, radiación); `APRSWX` sin indicativo usa `default_call`. Envío periódico opcional e independiente del METAR. Nueva sección `[aprs_wx]` (clave API gratuita de aprs.fi); sin clave la función queda desactivada. Se usa `APRSWX` y no `APRS` para dejar libre este último, p. ej. para consultas de posición.
- **Precisión de portadora y radios Hytera (PR #39, David Harcus, MM7FDM).** Las placas SDR clónicas pueden transmitir uno o dos kilohercios fuera de frecuencia; las Sepura registran igual, pero las **Hytera** (PT580H y similares) nunca encuentran la celda (antena tachada, ni una ráfaga de acceso en el registro de la estación) y ningún ajuste del codeplug lo arregla. Nueva herramienta `tools/carrier-offset.py`: barre la portadora con un RTL-SDR en cualquier ordenador cercano, localiza el canal TETRA de 25 kHz, informa del desvío y propone el `ppm_err` que lo centra (límite ETSI 0,2 ppm, menos de 100 Hz a 438 MHz). Documentación en `Docs/carrier-accuracy.md` y en el README; la ayuda del campo PPM del panel lo explica. Visto en un clon B210: portadora 1,56 kHz baja, dos Sepura registradas, una PT580H Plus muda hasta corregir la portadora a menos de 50 Hz.
- **Varias redes en una estación (`[networks]`, PR #38, David Harcus, MM7FDM) — probado al aire el 2026-11-07** (una celda `via` 901/1638 junto a una celda 901/9999 en una estación). Una estación puede servir varias redes TETRA a la vez, cada una con su propia identidad (MCC/MNC) y con su propio núcleo Brew o sin ninguno. Cada celda se vincula a una red (`network = "…"` en la raíz para la celda principal y en cada `[[cells]]`) y emite la identidad de esa red, de modo que al elegir una red en la radio el usuario cae en las celdas de esa red y se encamina a su núcleo: por ejemplo, una portadora en TetraPack (901/9999) y otra en otro núcleo (901/1234). Una red sin núcleo es local: sus celdas se conmutan dentro de la estación (llamadas de grupo e individuales, SDS e itinerancia entre ellas) sin ningún servidor. Cada red con más de una celda tiene su propio conmutador de sitio, alojado en la celda principal cuando esta sirve la red y, si no, en la celda de menor número de la red. Las celdas hermanas de otra red se anuncian como vecinas con su MCC/MNC. La tarjeta Celdas muestra la red de cada celda y el estado de su enlace; `CellsSnapshot` incorpora `network` y `link_up`. Sin la sección `[networks]` nada cambia. Documentación en `Docs/networks.md`. Pendiente: encaminar por la red de origen de la radio en una sola portadora (MNI en U-LOCATION UPDATE DEMAND) y validar con dos portadoras y una radio con ambas redes programadas.
- **Multi Network: una red alcanzada a través del servidor Brew (`via`), editable en el panel.** Probado en el aire (radio que selecciona 901/1638 en una celda `via`, registrada en TetraPack a través del brew-server). Se basa en «Varias redes en una estación» (PR #38, David Harcus, MM7FDM). Una entrada `[networks.<nombre>]` con `via = "<red>"` en lugar de su propio `[brew]` se alcanza a través del servidor Brew de esa red (Multi Network «Infraestructura»): sus celdas emiten su propio MCC/MNC (ETSI TS 100 392-2 tabla 18.17), se conectan con el usuario de la otra red y nombran sus radios y grupos con esta red ante un brew-server que ofrezca `mni`, que los registra allí, p. ej. en su enlace TetraPack. Así una radio con red propia 901/9999 que selecciona 901/1638 se registra en esa celda y funciona en 901/1638; la estación no guarda credenciales de esa red. Nueva tarjeta «Redes» en la página Infraestructura (nombre, MCC/MNC, acceso: servidor propio, vía, local) y columna «Red» por celda; la primera red añadida convierte `[net_info]` + `[brew]` en la red `home`, y al quitar una red sus celdas vuelven a la red de la celda principal en el mismo guardado. Una radio de la red propia del servidor también puede migrar a esa celda (con `[multi_network]` activo; probado al aire el 2026-11-07). Con varias redes solo las celdas de una red cuentan como portadoras de su estación para el roaming, y el conmutador de una red alojado en una celda adicional usa esa celda como respaldo.
- **Traspaso sin interrupción de llamadas individuales (privadas/dúplex) — provisional, aún sin validar al aire.** Una llamada privada o dúplex ya no tiene por qué cortarse cuando la radio cambia de celda a mitad de llamada: el conmutador del sitio pre-provisiona la pata restaurada en la celda destino (con canal de tráfico asignado y abierto) y el CMCE la empareja por par de ISSI, respondiendo al `U-CALL RESTORE` con `D-CALL RESTORE` y asignación de canal para llevar a la radio a la nueva portadora. **Pendiente:** reencaminado de la voz en el conmutador y limpieza del traspaso abortado; sin validar todavía en hardware multi-celda. *(Completado en v0.5.6 para llamadas con un participante Brew y entre dos radios de la estación, anunciado o no; ver «Multi-celda» allí.)*
- **Ticks de entrega de SDS en la consola.** Los informes de entrega SDS-TL que una radio envía al ISSI del despachador (o a 9999) ya no se descartan como «no registrado localmente»: se asocian al mensaje enviado por su referencia de mensaje y el registro SDS y la nueva vista «Enviados» de la consola muestran ✓ enviado, ✓✓ entregado o ✗ rechazado. Las entradas del registro guardan `message_ref` y `delivered`; nuevo evento de telemetría `SdsReport`.
- **Nombres de radio.** Botón «Nombre» en la tabla de radios y en la lista de la consola de despacho para asignar un nombre a un ISSI (guardado en `radio_names.json` junto a la configuración, API `GET/POST /api/radio-names`). El nombre se muestra donde aparezca ese ISSI, por delante del indicativo de RadioID, y el chip del grupo en recepción indica quién ha pulsado el PTT («▶ Dave (2358245)»); el estado de despacho incluye `rx_issi`.
- **Lista de escaneo con nombres e importación desde archivo (consola de despacho).** Cada TG puede llevar un nombre y se muestra como «World Wide (91)» en los chips y en la franja de llamada. Botón «Importar archivo» (CSV, TXT o JSON: `World Wide (91)`, `World Wide,91`, `91,World Wide` o `{"name":"World Wide","gssi":91}`) y «Exportar» a CSV; campo de nombre opcional al añadir un TG a mano.
- **Corrección: numeración de ranuras del enlace ascendente tras «Stack is N slot(s) behind the SDR clock».** Al adelantar el reloj de la pila para alcanzar al SDR, los demoduladores de subida no se movían con él, por lo que desde ese momento cada ráfaga de las radios se etiquetaba una ranura tarde: la voz de las radios caía en una ranura sin circuito (temporizador de inactividad a los 3 s de cada llamada, nadie oía a nadie en el grupo), las llamadas privadas no llegaban a establecerse y los SDS entre radios nunca se reensamblaban hasta reiniciar. Ahora la pila y los demoduladores avanzan juntos: las ranuras que el demodulador salta por pérdida de muestras se replican en la pila sin esperar a RX, y cualquier otro adelanto de la pila salta también una ranura de aire en los demoduladores de subida.
- **Autenticación de radios (TAA1, EN 300 392-7 cláusula 4).** La estación puede desafiar a cada radio con su clave K de 128 bits al registrarse (`[security] authentication = off | optional | required`, con autenticación mutua). Las claves se guardan por ISSI en `[[security.subscribers]]`. Nuevo crate `tetra-security` con HURDLE-II, TA11/TA12/TA21/TA22/TA41/TA51/TA52/TA61 y TB4/TB5, comprobado contra vectores de prueba públicos. Las radios que pasan muestran la insignia **AUTH** en la tabla de radios.
- **TEA2 y TEA3** además de TEA1 para el cifrado de clase 2 (`ksg = 2 | 3`). TEA1 se mantiene para investigación e interoperabilidad y se marca como débil (solo conserva 32 bits de clave).
- **OTAR de SCK (cláusula 4.5.2).** Entrega de la clave estática a una radio por el aire, sellada con la K de esa radio (TA41 + TA51); la respuesta de la radio (U-OTAR SCK RESULT) se muestra en el panel. Con `class = 1` y una clave configurada, la clave queda preparada solo para OTAR y la celda sigue en claro, de modo que la flota puede recibir la clave antes de activar el cifrado.
- **Página Seguridad** en el panel: estado en ejecución frente a guardado con reinicio en un clic, modo de autenticación, tabla de claves de abonado con generador de claves, cifrado (algoritmo, SCK, número, versión, grupos en claro), «Enviar SCK» por radio y a todas las radios conectadas; columna **Seguridad** en la tabla de radios y fila SEGURIDAD en la barra lateral.
- **USRP B210 (UHD):** opción «Instalar USRP (UHD)» en el asistente y en el instalador, valores por defecto correctos para full-duplex (TX en TX/RX, RX en RX2, PGA 40 dB), imagen FPGA abierta para los clones Kintex-7 (`fx3 is in state 5`), dependencias del paquete .deb y documentación.
- **Cifrado extremo a extremo (E2EE) de SDS.** Desactivado por defecto (`[security.e2ee]`): sella el contenido de los mensajes de estado y texto entre radios con AES‑128/256 en modo CTR y MAC AES‑CMAC (cifra‑luego‑autentica), independiente del cifrado de aire TEA. Pensado para sistemas privados; en la red amateur se incluye como función aunque no se use.
- **Página WAP con estado en vivo.** La página del punto de acceso muestra actividad real —SDS vistos, nivel de señal general, radios registradas y antigüedad de la última actividad de radio— con documentación de configuración para operadores. Se añade una **tabla por radio** que separa la actividad de grupo de la individual/privada.
- **Corrección: canal de datos por paquetes (PDCH) del WAP, PR #37, David Harcus, MM7FDM — provisional, aún sin validar al aire.** Al conceder el canal de datos, la estación movía la radio a una ranura que seguía anunciándose como no asignada y enviaba la respuesta WAP por el canal de control principal, que la radio ya no escuchaba, por lo que la página no llegaba. Ahora la ranura se anuncia como canal de control asignado mientras haya una radio en ella, y todo lo dirigido a esa radio (respuesta WAP, acuses de enlace, llamadas individuales y SDS entrantes) se envía por el canal de datos. El PDCH pasa a la ranura 4 para no chocar con la voz, que se asigna desde la ranura 2 y siempre tiene prioridad. Se responde a SN-RECONNECT, de modo que una sesión de datos continúa tras cambiar de celda.
- **Corrección: desactivar el WAP ya no impide arrancar la estación.** `[cell_info.wap_ip].enabled` pasa a ser el único interruptor maestro del WAP; con el WAP desactivado la BTS arranca con normalidad (antes fallaba al iniciar).
- **Mapa de posición (LIP) sin conexión.** La biblioteca del mapa (Leaflet) se sirve desde la propia estación, de modo que el mapa de ubicaciones funciona sin acceso a Internet.
- **Un error de configuración ya no detiene una estación en marcha.** Al cargar las celdas adicionales, una celda con configuración inválida se omite con un aviso en lugar de abortar el arranque de toda la estación.

## v0.5.4 — Escucha de ambiente (SS-AL) y petición de posición

- **Escucha de ambiente (SS-AL, ETSI EN 300 392-12-21).** Nueva función de despacho: la estación acepta el comando `AmbienceListen` del canal de control (brew-server) y reconoce el byte de servicio de escucha de ambiente que envía la consola. Monta una llamada individual directa y símplex hacia la radio destino con el indicador de notificación «AL operation» (valor 3, EN 300 392-9), de modo que una radio compatible conecta y abre micrófono por sí sola, sin acción del usuario. La radio **señaliza la llamada** como cualquier otra (no es encubierta). El audio de la radio llega a la consola y, al ser una llamada solo de recepción, ya no se corta a los 30 s. `enable: false` la libera. Validado al aire con MTH800.
- **Petición de posición (LIP).** El despacho puede pedir a una radio su posición (una vez o de forma periódica) y el informe LIP llega a la consola que lo solicitó. Antes, con el reenvío LIP activo, el informe se enviaba solo al ISSI de reenvío y la consola que lo había pedido no lo recibía.

## v0.5.2 — Multi-celda: RF de cada celda y versión en la telemetría

- **Página RF por celda.** Cada celda adicional envía ahora su espectro, constelación, cascada, calidad de señal (EVM, PAPR, fuga de portadora, ancho de banda) y salud del SDR (temperatura, ganancias). Pestañas compactas arriba de la página RF eligen la celda; cada celda conserva su propio historial de cascada.
- **Versión de la estación en la telemetría.** Nuevo evento `StationVersion` (versión, build y versión base) al conectar con el servidor de telemetría; brew-server la muestra junto a la IP de la BTS.
- **Menos tráfico de telemetría.** El espectro y la constelación (~5 KB, cinco veces por segundo y celda) ya no se envían al servidor de telemetría; solo los usa la página RF local. EVM, PAPR y salud del SDR se siguen enviando, por celda (`CellRf`).

## v0.5.1 — Multi-celda: SDS de red y telemetría de celdas

- **SDS de la red a radios de celdas adicionales.** Un SDS llegado por Brew (incluido el SMS Center de brew-server) para una radio registrada en una celda adicional se descartaba sin respuesta; ahora se entrega en su celda y Brew recibe el `SDS_REPORT`.
- **SDS entre celdas registrado una sola vez.** La celda que lo recibe de otra celda ya no lo vuelve a anotar como SDS de red en el registro de SDS ni en la telemetría.
- **Telemetría de celdas.** Nuevo evento `CellsSnapshot` cada 10 s (también en estaciones de una celda): portadoras y frecuencias, código de color, área de localización, vecinas, SDR, estado RF y las radios registradas en cada celda. brew-server lo muestra en su panel de telemetría y resincroniza con él su lista de registros tras una reconexión.

## v0.5.0 — Multi-celda: una estación, varios SDR

Primera versión multi-celda. Una estación, varios SDR: cada SDR es una celda TETRA más (p. ej. una Pi con dos Pluto+). Completo en código y tests, **aún sin validar en el aire**; las configs de una sola celda funcionan igual que en v0.4.4.

**Configuración y arranque**
- Nuevas entradas `[[cells]]` (id 1-7): heredan `[cell_info]`, llevan su propio `[cells.soapysdr]`. Portadoras únicas, mismo `freq_band` y `custom_duplex_spacing`, y `device` obligatorio en cada celda (también la principal). Las configs de una sola celda no cambian.
- Cada celda corre su propio stack de radio en su propio hilo y SDR.
- Dashboard: tarjeta **Celdas** en Inicio (estado RF, portadoras, SDR, radios) con escaneo de SDR, alta y baja de celdas (reinicio).

**Celdas enlazadas (con Brew o LST activos)**
- Llamadas de grupo, de la red o de cualquier radio, en todas las celdas con miembros; un solo hablante por grupo en todo el sitio; una llamada de emergencia se impone al hablante de otra celda.
- Llamadas individuales y SDS entre celdas; el SDS de grupo llega a los miembros de todas las celdas. Solo sale a Brew lo que no es para nadie del sitio; los grupos de `local_ssi_ranges` enlazan celdas sin salir a Brew.
- Asterisk (SIP) y WX/METAR desde todas las celdas; los cambios de WX en el dashboard se aplican a todas.
- La voz entre celdas pasa por un jitter buffer y sale al ritmo TDMA de la celda que la recibe.

**Movilidad**
- Cada celda anuncia a las demás como vecinas (reselección anunciada y no anunciada), con extensión de portadora y potencia máxima cuando difieren.
- Al registrarse en otra celda, la radio se da de baja en silencio en la anterior y restaura su llamada de grupo en la nueva.
- Traspaso anunciado: U-PREPARE → D-NEW-CELL (o D-PREPARE-FAIL); la celda destino se une antes a las llamadas de grupo de la radio; el registro reenviado en U-PREPARE (tipo 1) se procesa en la celda destino y su respuesta va dentro de D-NEW-CELL; U-RESTORE → D-RESTORE-ACK / D-RESTORE-FAIL.

**Dashboard y telemetría**
- La tabla de radios registradas muestra todas las celdas (insignia C0/C1…) y la página de llamadas incluye las de todas las celdas.
- Telemetría: nuevo evento `MsCell` (celda de cada registro); las llamadas de celdas adicionales usan identificadores propios de la estación (desde 0x4000).

**Cambios para todas las estaciones (también de una celda)**
- La estación ignoraba todas las PDU MLE de subida; ahora responde a U-PREPARE (D-PREPARE-FAIL si no hay vecinas) y a U-RESTORE.
- D-NWRK-BROADCAST con vecinas lleva umbrales/histéresis de reselección configurables (`[cell_info.cell_reselect]`, por defecto 20/10/10/6 dB); antes siempre 0.
- Tests de `tetra-entities` reparados (import `CallOrigin`, config Brew en el test de preempción) y `Cargo.lock` sincronizado.

**Pendiente de comprobar en el aire:** el orden de bits de los parámetros de reselección y de la extensión de portadora; llamadas individuales que no sobreviven a un cambio de celda; CPU/ancho de banda con varios SDR en una Pi.

## v0.4.4 — Puertos del dashboard: presets y binds estables

Elección de puertos sin pelear con nginx/apache ni spamear ERROR en el log.

- Instalador (SSH/TTY): pregunta **Estándar (80→443)** o **Puerto alto (solo HTTPS 8443)**; `BOST_DASH_PORTS=standard|high` sin TTY. Configs existentes no se tocan.
- Sistema → Acceso al panel: selector de preset + Apply & Restart (nueva URL tras reinicio).
- `port = 0` desactiva el redirect HTTP. Migración OTA solo si `port = 8080` sin `https_port`.
- Listeners legacy 8080/8443 solo en layout :443, fail-soft (sin reintentos infinitos). Bind canónico: conflicto → hint + retry 60s.
- Validado en campo. Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.4.3 — LST SDS: ocultar ACK de entrega en el log

- Los SDS-TL SHORT REPORT (confirmación de entrega del MS) ya no se registran en el log/inbox SDS.
- Evita la fila fantasma `[text]` justo después de un SDS enviado desde el despacho.
- Validado en campo (MS↔despacho privado/grupo, origen `operator_issi`). Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.4.2 — LST SDS: identidad despachador y ACK

SDS de despacho LST deja de fingir entrega a 9999 y usa la ISSI del operador.

- Con LST activo, SDS del roster/panel salen con `source_issi = operator_issi` (no 9999).
- SDS entrantes al `operator_issi` se absorben con SDS-TL SHORT REPORT (el walkie deja de marcar error de envío).
- Se elimina la absorción ciega de SDS-DATA a 9999: ruta estándar (local / Brew / undeliverable). WX y U-STATUS a 9999 sin cambios.
- Inbox LST: solo privados destinados al despachador (sin filtro dual 9999).

## v0.4.1 — Brew WiFi: leave mid-QSO como Ethernet

Misma lógica Ethernet/WiFi; con latencia WiFi el MS podía seguir como owner Local mientras Brew hablaba, y rojo/cambio de TG tumbaba el circuito (media huérfana + U-SETUP encima).

- Al preempt de Brew, ownership Local→Network.
- U-DISCONNECT del owner solo soft-leave si Brew tiene el suelo (D-RELEASE personal; grupo vivo).
- Sin listeners + Brew activo → Hold / LATE ENTRY (también si origin aún Local).
- Release Local con brew_uuid → NetworkCallEnd antes de cerrar circuito (anti-zombie DL).
- Owner con suelo local: teardown ETSI sin cambios.
- Validado en campo (WiFi Hold → LATE ENTRY). Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.4.0 — Red host, Dual Carrier GUI y canal estable

Salto menor de serie (aún sin rebrand a PTBS): nuevas capacidades de red en la GUI y consolidación de lo validado en beta.

### Página Red (antes WiFi)

- Menú **Red** con icono híbrido Ethernet+WiFi (superpuestos, estilo LST).
- Sección **Enlaces**: interfaces ethernet/wifi con IP(s) y badge de **ruta por defecto**.
- Gestión de perfiles **Ethernet** (conectar / desconectar) vía NetworkManager.
- WiFi (conexión actual, redes guardadas, disponibles) en **una sola tarjeta** con separadores.
- Estados NM y perfil «Wired connection» traducidos al idioma de la GUI.
- API `/api/network/*` (overview + ethernet); `/api/wifi/*` sin cambios.

### U-STATUS (walkie → ISSI 9999)

- `ip` / `info` listan **todas** las IPs de host (`eth0=…*`, `wlan0=…`; `*` = ruta por defecto).
- Respuesta **multilínea** (CR/LF admitidos en SDS de texto).
- Enumeración rápida con `getifaddrs` (sin `nmcli` en el hilo de radio — evita caída del stack).

### Dual Carrier (TMO Cell)

- Configuración Dual Carrier en GUI (Config → Advanced RF), límite al passband de Fs.
- Home / BTS Details: estado Activo/Apagado, mini-tiles secondary, orden MCCH/BCCH.

### Repo / OTA

- Eliminado el workflow de sync con upstream FlowStation (force-push a `main`).
- Promovido a canal **stable** (`main` / `bost`); `beta` al mismo tip.

## v0.3.47 — U-STATUS IP multilínea

- El SDS de texto admite CR/LF (antes se filtraban).
- Status IP: `Host IP` y cada interfaz en su línea (`eth0=…*`, `wlan0=…`).
- Página Red + fixes U-STATUS IP (v0.3.44–0.3.47) promovidos a canal **stable** (`main` / `bost`).

## v0.3.46 — Fix: U-STATUS IP no bloquea el stack

- El Status/info de IP ya no llama a `nmcli` en el hilo de radio (provocaba «Too late to produce TX block» y caída del stack).
- Lista IPs con `getifaddrs` + `primary_ip()` (rápido, como el Status de temperatura).

## v0.3.45 — Red: icono, i18n NM y WiFi unificado

- Icono de menú Red: jack Ethernet y arcos WiFi superpuestos (estilo LST), arcos más anchos.
- Sección de página «Red» (antes «Integraciones»); WiFi en una sola tarjeta con separadores.
- Estados NM (`connected`, …) y perfil «Wired connection» traducidos al idioma de la GUI.

## v0.3.44 — Página Red: Ethernet + WiFi

- La pestaña **WiFi** pasa a llamarse **Red** (icono híbrido Ethernet+WiFi).
- Nueva sección **Enlaces**: todas las interfaces ethernet/wifi con IP y badge de **ruta por defecto**.
- Gestión de perfiles **Ethernet** (conectar / desconectar) vía NetworkManager.
- El bloque WiFi (conexión, guardadas, escaneo) se mantiene debajo.
- U-STATUS `ip` / `info` listan las IPs por interfaz (`eth0=…* wlan0=…`; `*` = ruta por defecto).

## v0.3.43 — Dual Carrier: bordes de mini-tiles + promoción estable

- Marcos de Carrier / TX / RX / Duplex dentro de Dual Carrier un poco más oscuros (mejor contraste).
- Dual Carrier GUI (v0.3.38–0.3.43) promovido a canal **stable** (`main`).

## v0.3.42 — Dual Carrier: mismo fondo que el resto

- La tarjeta Dual Carrier usa el mismo fondo plano (tema light) que Registration Access / tiles BTS.

## v0.3.41 — Dual Carrier: una sola tarjeta

- Carrier / TX / RX / shift del secondary van **dentro** de la tarjeta Dual Carrier (sin caja aparte).
- Estado: **Activo** (verde negrita) / **Apagado** (naranja negrita); se quita el texto «carrier secundario #…».

## v0.3.40 — Dual Carrier UI: orden TS + mini-tiles

- En BTS Details, la fila **MCCH (main)** va arriba y el **BCCH secondary** debajo.
- Las mini-tiles del secondary (carrier, TX, RX, shift) aparecen **bajo Dual Carrier** cuando está activo (Home ya las rellena).

## v0.3.39 — Fix OTA: encoding en server.rs

- Corrige literales UTF-8 corruptos en el detector de mojibake del dashboard que impedían compilar `tetra-entities` en OTA (v0.3.38).

## v0.3.38 — Dual Carrier en TMO Cell (GUI)

- Dual Carrier se configura en **Config → Advanced RF** (checkbox bajo Main carrier; el secondary solo aparece si está ON).
- El secondary se **limita al passband** de la Fs real del SDR (o 600 kHz por defecto); al activar se guardan Fs + centros midway en TOML y perfil Cell.
- En **TETRA BTS Details**: botón «Configurar…» (sin switch) y **mini-tiles** del secondary (nº, TX, RX, shift) cuando está activo.

## v0.3.37 — Overflow mid-report → D-ATTACH inmediato

- Si el U-ATTACH trae *group report not complete* y >12 GSSI (MXP600), tras el ACK de 12 se afilia el resto con **D-ATTACH SwMI** en el acto (no se pide otro group report).
- Si el amendment siguiente llega truncado (`BufferEnded` al parsear), se usa el resto guardado del PDU anterior para el mismo D-ATTACH.

## v0.3.36 — Fallback SwMI attach si el MS no multipasa

- Tras group report (§16.8.3), si el MXP600 vuelve a mandar >12 GSSI en un solo U-ATTACH (no hace amendment multipaso), la BTS afilia el resto y envía **D-ATTACH amend** SwMI (§16.8.1) con esos GSSI. IOP puede ignorarlo; si sigue en 12, limitar scan a ≤12.

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
