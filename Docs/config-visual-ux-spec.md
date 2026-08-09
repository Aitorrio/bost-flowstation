# Spec UX — Configurador visual y perfiles

Inspirado en `BTS Mia/bts-web/static/settings.html`, integrado en el dashboard FlowStation v0.4 (`page-config`).

## Objetivos

1. Operar RF / red TETRA / Brew **sin editar TOML a mano**.
2. Catálogos ortogonales: **Cell profiles** × **Brew profiles**.
3. Apply explícito → materializa `config.toml` → restart.
4. Mantener el editor TOML raw como escape hatch (colapsado).

## Pantalla Config (orden)

```
[Profiles]     Cell profile ▾   Brew profile ▾   [Apply & Restart]  [Save as…]
[RF]           TX/RX Hz, PPM, device, antennas, gains, Auto carrier
[Network]      MCC/MNC, LA, colour, duplex, services, hangtime, T351…
[Brew]         enable, host, port, tls, user, password, reconnect…
[Access…]      whitelist / integraciones existentes (sin cambio)
[Advanced]     <details> editor TOML crudo
```

## Catálogos

### Cell profile (`profiles/cell/<name>.json`)

JSON con subárboles:

```json
{
  "phy_io": { "backend": "SoapySdr", "soapysdr": { "tx_freq": …, "rx_freq": …, … } },
  "net_info": { "mcc": …, "mnc": … },
  "cell_info": { "freq_band": 4, "main_carrier": …, "duplex_spacing": …, … }
}
```

Sin sección `brew`. Campos Station/Integrations no se guardan aquí.

### Brew profile (`profiles/brew/<name>.json`)

```json
{
  "host": "core.example",
  "port": 3003,
  "tls": true,
  "username": 28924,
  "password": "…",
  "reconnect_delay_secs": 15,
  "feature_sds_enabled": true,
  "feature_rssi_export": false,
  "whitelisted_ssis": null,
  "pbx_gateway_issis": null,
  "jitter_initial_latency_frames": 0
}
```

Selección especial **Sin Brew / Offline** → al apply se elimina `[brew]` del TOML.

### Activo (`profiles/active.json`)

```json
{ "cell": "Default", "brew": "MNO-LAN" }
```

`brew: null` = offline.

## Flujos

### Editar formulario vivo

1. `GET /api/visual-config` → rellena formularios desde TOML actual (password masked).
2. `POST /api/visual-config` → merge campos en TOML → validate → `.bak` → write.
3. Banner: “Reinicia para aplicar RF/red/Brew”.

### Guardar como perfil

- **Save Cell profile**: captura formularios RF+Network → `POST /api/profiles/cell`.
- **Save Brew profile**: captura formulario Brew → `POST /api/profiles/brew`.

### Apply & Restart

1. Usuario elige Cell + Brew (o Offline).
2. `POST /api/profiles/apply` `{ "cell": "Lab", "brew": "MNO-LAN"|null, "restart": true }`.
3. Backend: merge en TOML vivo (preserva dashboard/security/integraciones) → validate → write → opcional `restart` vía canal de control existente (el frontend ya usa `wsSend({type:'restart'})`).

### First-run

Si no hay perfiles: importar Cell desde TOML actual (sin brew) como `"Default"`; si hay `[brew]`, crear brew profile `"Default Brew"` y marcar activo.

## Auto (RF)

Botón junto a `main_carrier` / `rx_freq`:

- Requiere `freq_band == 4`, `reverse_operation == false`, TX Hz y duplex (tabla o custom).
- `main_carrier = (tx - 400_000_000 - offset) / 25_000` (entero).
- `rx_freq = tx - duplex_hz`.

## APIs

| Método | Path | Rol |
|---|---|---|
| GET | `/api/visual-config` | Formulario tipado actual |
| POST | `/api/visual-config` | Guardar formulario → TOML |
| GET | `/api/profiles/cell` | Listar |
| GET/POST/PUT/DELETE | `/api/profiles/cell/<name>` | CRUD |
| POST | `/api/profiles/cell/<name>/duplicate` | Duplicar |
| GET | `/api/profiles/brew` | Listar |
| GET/POST/PUT/DELETE | `/api/profiles/brew/<name>` | CRUD |
| GET | `/api/profiles/active` | Selección activa |
| POST | `/api/profiles/apply` | Combinar + escribir TOML |

## Persistencia

Directorio hermano al `config.toml`:

```
<config_dir>/profiles/cell/*.json
<config_dir>/profiles/brew/*.json
<config_dir>/profiles/active.json
```

Sin SQLite (nativo Rust, portable al `.deb`).

## i18n

Claves EN/ES mínimas en el diccionario del dashboard (`cfg_visual_*`, `profiles_*`).

## Fuera de alcance v1

- Perfiles RF y Network **separados** (v1 = un Cell profile conjunto, como BTS Mia).
- Editor tipado de vecinos CA / SDS command control / Asterisk (siguen en TOML o cards existentes).
- Apply en caliente sin restart.
