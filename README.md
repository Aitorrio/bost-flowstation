<div align="center">

<img src="contrib/logo/bost_flowstation_header.png" alt="Bost FlowStation" width="420"/>

### Software-defined TETRA base station (fork), built in Rust for Raspberry Pi.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Branch](https://img.shields.io/badge/branch-bost-00d4a8.svg)](https://github.com/Aitorrio/bost-flowstation/tree/bost)

</div>

---

## What is Bost FlowStation?

**Bost FlowStation** is a fork of [FlowStation](https://github.com/razvanzeces/flowstation) focused on **easy Raspberry Pi installs** and **day-to-day operation from the web dashboard** — without living in SSH and `config.toml`.

Based on FlowStation by **Razvan Zeces / YO6RZV** (itself built on [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation)). Improved by **Aitor, EA4HBL**.

**Tested hardware:** LimeSDR Mini 2.0 · SXceiver · Motorola MXP600 · Motorola MTM800E · Motorola MTM5400

> Protocol details, Brew theory, Asterisk/DAPNET/GeoAlarm and upstream bug history live in the [original FlowStation README](https://github.com/razvanzeces/flowstation/blob/main/README.md). This page documents **what this fork adds** and **how to run it**.

---

## What’s new in this fork

| Capability | Why it matters |
|---|---|
| One-command Pi install | Boots with RF off so the dashboard is always reachable |
| First-run **Setup** wizard | Install SDR drivers and finish RF / net / Brew from the browser |
| Visual **Config** (Cell × Brew profiles) | Switch networks without hand-editing TOML |
| Access control & U-STATUS remote control in GUI | Whitelist + walkie commands (`ip` / `temp` / `info` / `restart`…) without SSH |
| **System** control panel | Restart, shut down, and **OTA update** from the dashboard |
| Sidebar update badge | Glance notice when a newer `bost` commit is available |
| Spanish-first UI (multi-language) | Ready for operators who prefer ES |

---

## Installation

On **Raspberry Pi OS / Debian arm64**:

```bash
curl -fsSL https://raw.githubusercontent.com/Aitorrio/bost-flowstation/bost/contrib/install/install-bost.sh | sudo bash
```

When it finishes you should see something like:

<p align="center">
  <img src="Docs/screenshots/02-install-done.png" alt="Install finished — dashboard URL and default login" width="720"/>
</p>

| | |
|---|---|
| Dashboard | `http://<pi-ip>:8080` |
| Default login | `admin` / `1234` |
| Config on disk | `/etc/flowstation/config.toml` (kept across reinstalls) |
| Sources | `/opt/bost-flowstation` (branch `bost`) |

More detail (env vars, force-clean, helper script): [`Docs/install-and-setup.md`](Docs/install-and-setup.md).

---

## First login & Setup wizard

1. Open the dashboard URL printed by the installer.
2. Sign in with `admin` / `1234` (change the password later in Config).

<p align="center">
  <img src="Docs/screenshots/01-login.png" alt="Bost FlowStation login screen" width="420"/>
</p>

3. If setup is incomplete, the **Setup** wizard opens automatically (also available from the sidebar).
4. In **Select SDR**, scan hardware or install the **SXceiver** / **Lime** driver, then continue through RF, network and Brew. Enable RF and restart when ready.

<p align="center">
  <img src="Docs/screenshots/03-setup-sdr.png" alt="Setup wizard — Select SDR" width="640"/>
</p>

The stack starts with `phy_io.backend = "None"` so the web UI works even before an SDR is ready.

---

## Configure from the web UI

Open **Config** in the sidebar. Prefer the visual forms over raw TOML — that is the point of this fork.

### Cell × Brew profiles

1. Pick a **Cell** profile (RF + TETRA identity) and a **Brew** profile (backhaul).
2. Edit the forms below (frequencies, MCC/MNC, Brew host/user…).
3. **Update** the profile or **Save as** a new name.
4. **Apply & Restart** to put that Cell × Brew combination on air.

<p align="center">
  <img src="Docs/screenshots/08-profiles.png" alt="Config — Cell × Brew profiles" width="720"/>
</p>

Use this to jump between networks (e.g. local cell vs BrandMeister) without rewriting `config.toml` by hand.

### Live settings (RF & network)

Frequencies (TX/RX and custom duplex) are entered in **MHz** (comma or dot). On blur the UI normalizes like Motorola CPS to six decimals (e.g. `432,2` → `432.200000`); the running `config.toml` still stores **Hz**.

**Save live** writes the forms into the running `/etc/flowstation/config.toml`. Use **Restart** (also available under **System**) when RF / network / Brew need a service restart.

<p align="center">
  <img src="Docs/screenshots/09-live-settings.png" alt="Config — live settings, RF frequencies and network identity" width="720"/>
</p>

### Access control

In **Access Control**, the ISSI whitelist is part of the **selected Cell** form (same workflow as RF/network — not Brew). Empty list = open network for that Cell; with entries, only listed radios may register when that Cell is on air.

- **Update Cell** / **Save as** persist the list into the Cell profile.
- **Apply & Restart** puts the Cell (including its whitelist) on air.
- Existing Cell profiles without a `security` key are left alone on Apply until you Update that Cell — your current live `[security]` is not cleared by accident.

U-STATUS remote control stays station-wide (not stored in Cell/Brew).

<p align="center">
  <img src="Docs/screenshots/10-access-control.png" alt="Config — ISSI whitelist / access control" width="720"/>
</p>

### Remote control (U-STATUS)

In **Control remoto (U-STATUS)**, authorize radios and map status codes to actions (`ip`, `temp`, `info`, `restart`, `shutdown`, `kick_all`). Configured from the dashboard; not stored inside Cell/Brew profiles.

<p align="center">
  <img src="Docs/screenshots/11-remote-control.png" alt="Config — remote control via U-STATUS" width="720"/>
</p>

### Raw `config.toml`

Still available under **Advanced** for power users: **Save** + **Restart** only. Full annotated reference: [`example_config/config.toml`](example_config/config.toml).

<p align="center">
  <img src="Docs/screenshots/12-raw-config.png" alt="Config — raw config.toml editor with Save and Restart" width="720"/>
</p>

---

## Day-to-day dashboard

<p align="center">
  <img src="Docs/screenshots/04-sidebar.png" alt="Dashboard sidebar — monitor pages and live BS/Brew status" width="280"/>
</p>

| Page | Use it for |
|---|---|
| **Radios** | Registered terminals, RSSI, kick / SDS, timeslot view |
| **DGNA** | Assign / deassign talkgroups over the air |
| **Calls / Last Heard / Log** | Live traffic and diagnostics |
| **RF / Health** | Spectrum / constellation and subsystem health |
| **Config** | Profiles, Cell ISSI whitelist, remote U-STATUS, live save |
| **System** | Host metrics, service control, OTA |
| **Setup** | Re-run first-boot helper anytime |

---

## System control & OTA updates

All service actions live under **System → Control** (not in Config).

<p align="center">
  <img src="Docs/screenshots/05-system-control.png" alt="System — update banner and service control buttons" width="720"/>
</p>

- **Reiniciar** — soft restart of `bluestation-bs`
- **Apagar** — stop the service (needs a manual start afterwards)
- **Actualizar** — OTA: `git fetch` on branch `bost`, rebuild, install binary, restart

When a newer commit is on GitHub, a banner appears here and a matching badge shows in the sidebar (click → System).

The OTA dialog shows a progress bar and the current build line (expand **Ver todo** for the full log):

<p align="center">
  <img src="Docs/screenshots/06-ota-updating.png" alt="OTA update in progress with progress bar" width="520"/>
</p>

<p align="center">
  <img src="Docs/screenshots/07-ota-complete.png" alt="OTA update completed — restarting" width="520"/>
</p>

Compiles on a Pi can take several minutes — leave the window open until it finishes.

---

## Optional: build from source

Only if you are not using the installer:

```bash
git clone https://github.com/Aitorrio/bost-flowstation.git -b bost
cd bost-flowstation
cp example_config/config.toml ./config.toml
# Prefer phy_io.backend = "None" until RF is configured via Setup / Config
cargo build --release -p bluestation-bs
./target/release/bluestation-bs config.toml
```

---

## Branches

| Branch | Purpose |
|---|---|
| **`bost`** | This fork — install script, Setup, visual config, OTA, Spanish UI |
| `main` | Upstream-aligned mirror — **do not** use for Bost features |
| `alpha` | Upstream active development |

---

## Upstream & community

- **This fork:** [github.com/Aitorrio/bost-flowstation](https://github.com/Aitorrio/bost-flowstation) (branch `bost`)
- **Upstream project:** [FlowStation](https://github.com/razvanzeces/flowstation) · [flowstation.dev](https://flowstation.dev) · [Telegram](https://t.me/+fktnT-th7dcxYWNk)

For Asterisk, DAPNET, Snom, GeoAlarm and deep protocol notes, use the upstream docs — this fork inherits those features but documents the **Pi + dashboard** path here.

---

## Credits

- **Razvan Zeces / YO6RZV** and the FlowStation community for the base station stack
- **Mihajlo YU4MSH**, **Torben DJ2TH**, **Joaquin EA5GVK** and others credited upstream
- **MidnightBlueLabs** — [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation)
- **Aitor, EA4HBL** — this fork (install, Setup, visual config, OTA UX, Spanish UI)

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
