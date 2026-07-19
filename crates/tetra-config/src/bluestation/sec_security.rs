use serde::Deserialize;

/// How `issi_whitelist` is interpreted. A bare `Vec` cannot express "deny everyone": an operator
/// who empties the list to lock the cell down actually opens it fully under the legacy semantics.
/// This makes the posture explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhitelistMode {
    /// No mode configured — legacy semantics: an empty list means "open network", a non-empty
    /// list is an allow-list. Default so existing configs behave exactly as before.
    #[default]
    Auto,
    /// Access control off: every ISSI is allowed whatever the list holds.
    Open,
    /// The list is authoritative. An EMPTY list therefore means DENY-ALL — the only way to
    /// express "lock the cell down", which `Auto` cannot.
    Enforce,
}

impl WhitelistMode {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(WhitelistMode::Auto),
            "open" | "off" | "disabled" => Some(WhitelistMode::Open),
            "enforce" | "strict" => Some(WhitelistMode::Enforce),
            _ => None,
        }
    }
}

/// Default cap on the MM client registry. Uplink is unauthenticated (EN 300 392-7 TEA is not
/// implemented), so any radio can claim any of the 2^24 ISSIs — without a cap a registration
/// flood grows the registry until the cell is OOM-killed.
pub const DEFAULT_MAX_REGISTERED_CLIENTS: usize = 2048;
/// Default accepted registrations per minute, per source ISSI. Generous for a real radio (T351
/// plus a post-PTT roaming update), tight enough that one forged ISSI cannot churn the registry.
pub const DEFAULT_REGISTRATION_RATE_LIMIT_PER_MIN: u32 = 30;

/// Access control / security configuration
#[derive(Debug, Clone)]
pub struct CfgSecurity {
    /// ISSI whitelist. Interpretation depends on `whitelist_mode`.
    /// Example config:
    ///   [security]
    ///   issi_whitelist = [2260571, 1001, 1002]
    ///   whitelist_mode = "enforce"   # empty list = deny-all
    pub issi_whitelist: Vec<u32>,
    /// See [`WhitelistMode`].
    pub whitelist_mode: WhitelistMode,
    /// Honour an unauthenticated U-ITSI-DETACH / migrating location update as a teardown of the
    /// claimed ISSI. There is no air-interface authentication, so such a PDU is forgeable and a
    /// replay is a targeted DoS; an operator who does not need detach at all can switch it off.
    pub honour_unauthenticated_detach: bool,
    /// Hard cap on the MM client registry (0 = unlimited, pre-hardening behaviour).
    pub max_registered_clients: usize,
    /// Accepted registrations per minute per source ISSI (0 = disabled).
    pub registration_rate_limit_per_min: u32,
}

impl Default for CfgSecurity {
    fn default() -> Self {
        CfgSecurity {
            issi_whitelist: Vec::new(),
            whitelist_mode: WhitelistMode::Auto,
            honour_unauthenticated_detach: true,
            max_registered_clients: DEFAULT_MAX_REGISTERED_CLIENTS,
            registration_rate_limit_per_min: DEFAULT_REGISTRATION_RATE_LIMIT_PER_MIN,
        }
    }
}

impl CfgSecurity {
    /// Returns true if the given ISSI is allowed to register.
    pub fn is_issi_allowed(&self, issi: u32) -> bool {
        self.allows(issi, None)
    }

    /// Whitelist decision honouring an optional runtime (dashboard) override list, which replaces
    /// the configured list. The mode applies to whichever list is effective, so an operator who
    /// clears the list from the dashboard under `enforce` gets deny-all, not an open cell.
    pub fn allows(&self, issi: u32, override_list: Option<&[u32]>) -> bool {
        let list = override_list.unwrap_or(&self.issi_whitelist);
        match self.whitelist_mode {
            WhitelistMode::Open => true,
            WhitelistMode::Auto => list.is_empty() || list.contains(&issi),
            WhitelistMode::Enforce => list.contains(&issi),
        }
    }

    /// One-line description of the effective access-control posture, for the startup log. The
    /// whole point is that an operator can read the cell's real posture out of the log rather
    /// than inferring it from an empty TOML array.
    pub fn access_control_posture(&self) -> String {
        let n = self.issi_whitelist.len();
        match self.whitelist_mode {
            WhitelistMode::Open => "OPEN — access control disabled (whitelist_mode = \"open\")".to_string(),
            WhitelistMode::Auto if n == 0 => {
                "OPEN — no issi_whitelist configured; ANY ISSI may register (set whitelist_mode = \"enforce\" to lock down)".to_string()
            }
            WhitelistMode::Auto => format!("ALLOW-LIST — {n} ISSI(s) may register"),
            WhitelistMode::Enforce if n == 0 => "DENY-ALL — whitelist_mode = \"enforce\" with an empty issi_whitelist".to_string(),
            WhitelistMode::Enforce => format!("ALLOW-LIST (enforced) — {n} ISSI(s) may register"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CfgSecurityDto {
    #[serde(default)]
    pub issi_whitelist: Vec<u32>,
    #[serde(default)]
    pub whitelist_mode: Option<String>,
    #[serde(default)]
    pub honour_unauthenticated_detach: Option<bool>,
    #[serde(default)]
    pub max_registered_clients: Option<usize>,
    #[serde(default)]
    pub registration_rate_limit_per_min: Option<u32>,
}

pub fn apply_security_patch(dto: CfgSecurityDto) -> CfgSecurity {
    let defaults = CfgSecurity::default();
    // An unrecognised mode falls back to "auto"; the effective posture is logged at startup
    // (see access_control_posture) so a typo can't silently pass for a lockdown.
    let whitelist_mode = dto
        .whitelist_mode
        .as_deref()
        .map(|s| WhitelistMode::parse(s).unwrap_or(WhitelistMode::Auto))
        .unwrap_or(WhitelistMode::Auto);
    CfgSecurity {
        issi_whitelist: dto.issi_whitelist,
        whitelist_mode,
        honour_unauthenticated_detach: dto.honour_unauthenticated_detach.unwrap_or(defaults.honour_unauthenticated_detach),
        max_registered_clients: dto.max_registered_clients.unwrap_or(defaults.max_registered_clients),
        registration_rate_limit_per_min: dto
            .registration_rate_limit_per_min
            .unwrap_or(defaults.registration_rate_limit_per_min),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The footgun: an empty list must stay "open" under the legacy default, but `enforce` must
    /// make the same empty list mean deny-all.
    #[test]
    fn empty_whitelist_semantics_depend_on_mode() {
        let mut cfg = CfgSecurity::default();
        assert!(cfg.is_issi_allowed(1234), "empty list under auto = open network");

        cfg.whitelist_mode = WhitelistMode::Enforce;
        assert!(!cfg.is_issi_allowed(1234), "empty list under enforce = deny-all");

        cfg.issi_whitelist = vec![1234];
        assert!(cfg.is_issi_allowed(1234));
        assert!(!cfg.is_issi_allowed(5678));

        cfg.whitelist_mode = WhitelistMode::Open;
        assert!(cfg.is_issi_allowed(5678), "open ignores the list entirely");
    }

    /// The dashboard override replaces the list but not the mode.
    #[test]
    fn override_list_follows_the_configured_mode() {
        let mut cfg = CfgSecurity::default();
        cfg.issi_whitelist = vec![1];
        assert!(cfg.allows(2, Some(&[2])), "override list is authoritative");
        assert!(!cfg.allows(1, Some(&[2])), "config list is ignored when overridden");
        assert!(cfg.allows(9, Some(&[])), "empty override under auto = open");

        cfg.whitelist_mode = WhitelistMode::Enforce;
        assert!(!cfg.allows(9, Some(&[])), "empty override under enforce = deny-all");
    }
}
