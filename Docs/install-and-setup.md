# Install & first-run setup (bost-flowstation)

Automated install for Raspberry Pi OS / Debian **arm64**, plus a web Setup wizard so the station comes up **without an SDR** and you finish configuration from the browser (no SSH required after install).

## One-command install

On the Pi:

```bash
curl -fsSL https://raw.githubusercontent.com/Aitorrio/bost-flowstation/bost/contrib/install/install-bost.sh | sudo bash
```

Or from a local checkout of the `bost` branch:

```bash
sudo ./contrib/install/install-bost.sh
```

### What the script does

1. Installs build tools + SoapySDR tools/libs (and Lime modules if available in apt).
2. Clones or updates `/opt/bost-flowstation` (branch `bost`), unless `BOST_SRC` points at an existing tree.
3. Builds `bluestation-bs` with Cargo and installs it to `/usr/local/bin/bluestation-bs`.
4. Writes `/etc/flowstation/config.toml` if missing, with:
   - `phy_io.backend = "None"` (web always starts)
   - `[dashboard]` on `0.0.0.0:8080`, login `admin` / `1234`
   - `service_name = "bluestation-bs"`
   - sibling `config.toml.fallback` and `setup.json` (`setup_complete=false`)
5. Installs `bost-setup-helper.sh` + a sudoers drop-in (allowlisted actions only).
6. Enables and starts `bluestation-bs.service`.

### Useful environment variables

| Variable | Meaning |
|---|---|
| `BOST_SRC` | Existing source tree (skip clone) |
| `BOST_BRANCH` | Git branch (default `bost`) |
| `BOST_REPO` | Git URL |
| `BOST_SKIP_BUILD=1` | Reuse an already-built `target/release/bluestation-bs` |
| `BOST_SERVICE_USER` | User for rustup/build (default `bts`) |

## First login

1. Open `http://<pi-ip>:8080`
2. Log in with `admin` / `1234` (change password in Config when convenient)
3. The **Setup** wizard appears if `setup.json` has `setup_complete=false`
4. Steps: welcome → SDR scan / install driver (SXceiver or Lime) → RF/net/Brew (or defaults) → enable RF + restart → ensure systemd autostart → finish

You can also open the **Setup** sidebar tab at any time.

## Degraded boot (no SDR)

If `backend = "None"` or SoapySDR open fails, the process **keeps running**. The dashboard exposes RF state in `/api/system` (`rf_status`: `online` | `offline` | `error` | `starting`) and shows a banner when RF is not online.

## Privileged helper

The dashboard never runs free-form shell. Driver install and systemd ensure go through:

`/usr/local/sbin/bost-setup-helper.sh`

Allowed actions: `install-driver sx|lime`, `enable-service`, `restart-service`.

## Updating an existing Pi carefully

If a cell is already on air, prefer a maintenance window:

```bash
export BOST_SRC=/path/to/checkout   # optional
sudo BOST_SKIP_BUILD=0 ./contrib/install/install-bost.sh
```

The script **does not overwrite** an existing `/etc/flowstation/config.toml`. To force wizard again, set `"setup_complete": false` in `/etc/flowstation/setup.json`.
