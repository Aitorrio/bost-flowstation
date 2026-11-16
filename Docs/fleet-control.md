# Fleet Control (station side)

Fleet Control lets the Brew server (brew-server, the SwMI) manage this station
over the control channel (`[command]`). The full protocol and the server side
are in brew-server's `docs/Fleet-Control.md`; this page covers what the station
does. Phase 1 (foundation): the switch, the hello, the general answer and a
round-trip check. Later phases add update, restart, reading and changing the
config.

## Switch

- Infrastructure page, Brew line (above the Server features list, always
  visible) → **Fleet Control** ("Station
  management by SwMI (BrewServer)"). Shown only while the server offers
  `fleet-control` (in `X-Brew-Extensions` of the Brew connection or of the
  control channel). Default **off**.
- Stored as `[command] fleet_control = true` (absent = off). It works at once:
  the config file is written and the running station takes the new value, no
  restart.
- Sidebar, under Health: field **Fleet Control**, green "on" / red "off", only
  while the server offers it. The tooltip says when the control channel is not
  connected. A click opens the Infrastructure page.
- The existing control commands (kick, DGNA, SDS, live SDS, emergency clear,
  ambience listening, service restart / shutdown) do not depend on the switch.
- Switching off asks twice (the Fleet Control switch and the remote command /
  control switch at the bottom of the page alike): first what stops working,
  **I understand** left and a red **Cancel** right; then "are you sure, the
  system admin and the fleet managers are notified", **Cancel** left and a red
  **Yes, switch off** right. Buttons and colours swap between the two dialogs so
  a double tap cannot get through. Saving the command channel switched off
  while Fleet Control is on first switches Fleet Control off (live), so the
  server logs "switched off Fleet Control" before the channel goes away.

```toml
[command]
host = "brew.example.org"
port = 443
use_tls = true
username = "1000001"     # kept identical to the Brew login
password = "…"
fleet_control = true
```

## Control channel missing

Turning the switch on without a usable `[command]` (missing, no host, or
`enabled = false`) opens a dialog:

| Field | Default |
| --- | --- |
| Server (IP address or DNS name) | empty, never pre-filled |
| Port | 443 |
| TLS (wss://) | on |
| Username | the Brew login |
| Password | empty = take the Brew password (done on the station; the password never goes to the browser) |

Saving writes `[command]` (switched on) with `fleet_control = true`, checks the
whole config like every dashboard save, and restarts the service: the control
channel is started at boot.

## What the station sends

Only on a control connection whose HTTP 101 names `fleet-control`:

- `StationHello {protocol: 1, version, brew_login, mcc, mnc, fleet_control,
  capabilities}` right after connecting and whenever the switch changes (also
  to off, so the server can drop the station from its list and log it).
  `capabilities` is e.g. `["fleet-v1", "maintenance-v1", "restart-v1", "metrics-v1", "config-v1", "update-v1", "update-live-v1"]` while the switch is on
  (`update-v1` only while the dashboard runs, because the OTA lives there),
  empty while off. A hello is sent again when `update-v1` appears later.
- `CommandResult {handle, ok, code, message}` for every Fleet Control command.

Commands understood (`fleet-v1`):

- `FleetPing {handle}`: answered by the control worker with `CommandResult`
  `ok` / `fleet_control_off`.

Commands understood (`update-v1`, phase 2, remote update): see the next section.

A server that does not offer `fleet-control` (older brew-server, TetraPack)
never sees any of this.

## Remote update (phase 2, `update-v1`)

The server can check for and start the same OTA as the dashboard's Update
button. The station always updates from **its own OTA source**
(`[dashboard] ota_channel`, by default `installed` = the repository and branch
it was installed from); the server never chooses a repository or branch.

| Command (server → station) | Answer |
| --- | --- |
| `CheckUpdate {handle}` | `UpdateInfo {handle, ok, current, latest, update_available, channel, repo, branch, message}` when the check is done (fresh, no cache; it asks GitHub, so it can take some seconds) |
| `StartUpdate {handle, during_call, source?}` | `CommandResult` at once: `ok`, `busy` (an update is already running), `call_active` (a call is in progress and `during_call` is false), `unavailable` (no dashboard, so no OTA), `invalid_source` (live update only), `fleet_control_off`. With `source`: live update, see below |

`during_call` defaults to false. `repo` never carries credentials. `latest` has the form of the running version: the source's version number plus the commit of its tip (`v0.5.9-d081d936`), or `branch@commit` when the version number cannot be read.

While an OTA runs (whoever started it: server or dashboard), the station sends

```json
{"UpdateProgress": {"stage": "build", "percent": 42, "message": "   Compiling tetra-entities v0.1.0", "version": "v0.5.9-8d7a363", "elapsed_secs": 312}}
```

when the stage or the percentage changes and at least every 10 s, and once
more at the end:

| `stage` | Meaning | `percent` |
| --- | --- | --- |
| `download` | checking the source, `git fetch` | 5 |
| `build` | `cargo build`; **the radio is off the air from here on** (the OTA frees the SDR for the build, `rf_status::request_ota_rf_off`) | 10–90, estimated: compiled units against the last successful build (`target/.ota-build-units`, else the packages in `Cargo.lock`) |
| `install` | the new program is put in place | 92 |
| `restart` | built and installed, the service restarts in 5 s | 100 |
| `up_to_date` | nothing to do, the running build is the newest of the source | 100 |
| `failed` | stopped with an error (`message` = last log line); the old version stays or is restarted | 0 |

`version` is the software running now (the old one until the restart). After
the restart the station connects again and its `StationHello` carries the new
version: that is how the server knows the update finished.

A station that is already up to date answers `StartUpdate` with `ok` and then reports `up_to_date`; the server skips it.

The OTA state is one per process (`net_dashboard::ota_remote`): a run started
by the server shows on the dashboard and the other way round, and only one
run can be active.

### Live update (`update-live-v1`)

`StartUpdate` with a `source` asks for a **live update**:

```json
{"StartUpdate": {"handle": 12, "during_call": false, "source": {"repo": "https://github.com/sebastianmadl/bost-flowstation", "branch": "fleet-control"}}}
```

The station runs its installer, the same as
`sudo BOST_REPO=<repo> BOST_BRANCH=<branch> bash contrib/install/install-bost.sh`
in a terminal (`server.rs` `run_live_update`), and **keeps the radio on during
the build**. Only the service restart at the end (5 s after the install) takes
it off the air. Details:

- `repo` must be a plain `https://` URL without credentials and `branch` a plain
  branch name (letters, digits, `-._/`); anything else is refused with
  `invalid_source`. Both go to the installer as environment variables, never
  through a shell line. stdin is closed, so the installer never waits for input.
- The service runs with real-time scheduling (`CPUSchedulingPolicy=fifo` in the
  unit) and children inherit it. The installer is started as a normal task
  (`SCHED_OTHER`, nice 10), like a terminal session, so the build does not take
  CPU time from the radio.
- The installer sets `[dashboard] ota_channel = "installed"` and the git origin
  to `repo`, so the station's normal OTA follows this source afterwards.
- No check is needed first, and coming back with the same version counts as
  done (reinstalled).
- Progress: the same `UpdateProgress` stages, taken from the installer's log
  (`==> Building bluestation-bs` → `build`, `==> Installed` → `install`).
  A failed run leaves the old version running and the radio on.
- The station lists `update-live-v1` together with `update-v1`. A server must
  send `source` only to stations that list it: an older station ignores the
  field and would run a normal OTA (radio off) instead.

## Maintenance banner (`maintenance-v1`)

`SetMaintenance {handle, on, minutes, by, reason}` → `CommandResult` (`ok`,
`fleet_control_off`). While the flag is on, **every page** of the dashboard
shows a solid red banner under the top bar: "Maintenance: work on this base
station in progress", with who set it, the minutes left and the note
(`reason`). `minutes` (default 10, at most 240) is how long the flag stays on
without being sent again, so a closed browser on the server never leaves it on;
the server sends it again after the station reconnects (e.g. after a restart)
while its own copy has time left. The flag lives in memory only
(`net_control::fleet::maintenance`); it is shown only while Fleet Control is on.

A **live update** shows its own red banner on every page, without a flag:
"Live system update: RF on!" with the build percentage, until the service
restarts.

The dashboard reads both from `GET /api/fleet-control` (`maintenance`:
`{by, reason, left_secs}` or null; `live_update`: `{stage, percent}` or null),
every 5 s.

The banner also shows **without a login**: on the login page and on the public
overview. For that, `GET /api/fleet-banner` is open without a session (not on
the private HTTP listener, which stays fully gated) and returns only
`{maintenance, live_update}` as above: no server address, no login, no switch
state. The public overview falls back to it when `/api/fleet-control` answers
401; the login page polls it every 5 s.

## Restart (`restart-v1`)

`RestartStation {handle, host, during_call}` → `CommandResult`: `ok` (the
service restarts, `host` false, or the host reboots, `host` true, 5 s later),
`busy` (an update is running), `call_active` (a call is in progress and
`during_call` is false), `fleet_control_off`. The service runs as root
(`User=root` in the unit the installer writes), so the reboot
(`service_control::schedule_host_reboot`, `systemctl reboot`) needs no sudo or
polkit rule and works right after a fresh install. The server knows the
station is back from its next `StationHello`.

## Station config (`config-v1`)

Phases 4/5: the server reads the config and changes it with a rollback. The
capability is announced once `fleet_config::boot` knows the config path (at
start, in `main`).

- **Hash.** `config_hash` in the hello: md5 (16 hex) of the parsed TOML
  re-serialized, without `[command] fleet_control`, so comments, layout and
  the switch do not count. `config_pending`: seconds left of a change waiting
  for confirmation. The worker sends a new hello when either changes (checked
  every 5 s), so a local edit reaches the server.
- **`GetConfig`** → `ConfigInfo {hash, config, text, pending}`: `config` is
  the Infrastructure view (the same JSON as `GET /api/roaming`, without the
  server's cell directory) plus the Brew link, cell SDR devices and the OTA
  source, `home_mode_display` (the `[cell_info.home_mode_display]` table or
  null) and `access` (`issi_whitelist`, `whitelist_mode`); passwords only as
  `password_set`. `text` is the file with passwords
  and keys as `***`.
- **`SetConfig {base_hash, changes, confirm_secs, during_call, by}`**: refused
  with `conflict` (the file is not `base_hash`), `invalid` (unknown key or
  the result does not validate like the dashboard's own save), `call_active`,
  `busy` (update running or a change already waits). Accepted keys:
  `reselect`, `rxlev_access_min_dbm`, `migration`, `presence_check_secs`,
  `multi_network_enabled`, `networks`, `telemetry`, `command`, `cells`,
  `cells_add`, `cells_remove`, `brew`, `ota`, `net_info`, `raw`. `raw`:
  `[{path, value?, remove?}]`, single values of the file (the server's config
  editor, Home Mode Display and ISSI whitelist cards), applied last; paths
  through a password or key name and `***` values are refused. An empty password
  keeps the old one; a new carrier moves the cell's SoapySDR frequencies. On
  `ok` the station saves `<config>.fleet-backup`, writes
  `<config>.fleet-pending.json` (old/new hash, window, who, when), writes the
  new config and restarts 3 s later.
- **Window.** `confirm_secs` (default 180, 60–900) from the restart, at least
  20 s after the boot. A watchdog thread armed at boot (and at `SetConfig`)
  copies the backup back and restarts when it runs out.
- **`ConfirmConfig`**: removes backup and note (`ok`, `not_pending`). The run
  that wrote the change answers `not_restarted`: its re-announced hello
  already says "pending", but it still runs the old config.
- **`RollbackConfig`**: backup back, restart (`ok`, `not_pending`).
- Keys (`[security]`) are never part of `config` or `changes`.

## Metrics (`metrics-v1`, phase 7)

While the server offers `fleet-control` and the switch is on, the control
worker sends a report 5 s after the connect (or after the switch went on) and
then every 60 s:

```json
{"FleetMetrics": {"interval_secs": 60, "cpu_pct": 17.4, "cpu_temp_c": 52.1,
  "cells": [{"cell": 0, "rf": "online", "sdr_temp_c": 44.3},
            {"cell": 1, "rf": "error", "sdr_temp_c": null, "detail": "SoapySDR: no device found"}],
  "radios": 3, "calls": 1, "health": "ok"}}
```

- `cpu_pct`: busy share of all cores between two reports, from `/proc/stat`
  (idle includes iowait), one decimal; `null` on the very first sample.
- `cpu_temp_c`: the first thermal zone whose type names the CPU (`cpu-thermal`
  on a Raspberry Pi, `x86_pkg_temp`), else a hwmon CPU driver (`coretemp`,
  `k10temp`, `cpu_thermal`, `zenpower`), else the first thermal zone; `null`
  without a sensor.
- `cells`: one entry per cell (`rf_status`): `rf` is `online`, `off` (no SDR
  configured), `error` (the SDR could not be opened, `detail` says why) or
  `starting`; `sdr_temp_c` is the last `SdrHealth` temperature of that cell
  (the telemetry fan-out feeds it, read every 10 s by the PHY), `null` when the
  SDR has no sensor or did not report for 2 min.
- `radios`: registered radios (health registry), `calls`: calls in progress
  (dashboard), `health`: overall stack health (`ok`, `degraded`, `critical`).

No answer is expected. The server uses it for its graphs and alarms
(brew-server `docs/Fleet-Control.md` section 12); it also pings the station
with `FleetPing` for its ping graph.

## HTTP API (dashboard)

| Route | Body | Answer |
| --- | --- | --- |
| `GET /api/fleet-control` | – | `{offered, enabled, connected, command: "on"/"off"/"missing", brew_login, maintenance, live_update}` |
| `POST /api/fleet-control` | `{"enabled": bool, "command": {host, port, use_tls, username, password}?}` | `{ok: true, enabled, restart}`, or `{ok: false, need_command: true}` when the dialog is needed |

`fleet_control` with the same fields is also part of `GET /api/roaming`.

## Code

- `crates/tetra-entities/src/net_control/fleet.rs`: names, hello, answers.
- `crates/tetra-entities/src/net_control/worker.rs`: offer check on connect,
  hello on connect and on switch changes, `FleetPing`.
- `crates/tetra-entities/src/net_dashboard/fleet.rs`: switch, dialog, status.
- `crates/tetra-entities/src/net_dashboard/ota_remote.rs`: remote update (check,
  start, progress) on the dashboard's OTA (`server.rs` `run_update`,
  `update_check_json`, `UpdateState.stage`).
- `crates/tetra-entities/src/net_control/metrics.rs`: `FleetMetrics` (CPU load,
  temperatures, RF state), sent by `worker.rs` `report_metrics`; SDR
  temperatures are fed from the telemetry fan-out in `bins/bluestation-bs`.
- `crates/tetra-entities/src/net_control/fleet_config.rs`: config read, change,
  backup, watchdog, confirm / rollback (`config-v1`).
- `crates/tetra-config/src/bluestation/sec_control.rs` (`fleet_control`),
  `state.rs` (`FleetControlState`: enabled, offered, connected).
