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
    /// Air interface encryption (EN 300 392-7 clause 6). `None` = security class 1 (clear).
    pub aie: Option<CfgAie>,
    /// Air-interface authentication posture (EN 300 392-7 clause 4, TAA1).
    pub authentication: AuthenticationMode,
    /// Answer a radio's own challenge (U-AUTHENTICATION DEMAND) and, when we are challenged first,
    /// also challenge back so both sides are authenticated (clause 4.1.4).
    pub mutual_authentication: bool,
    /// Authentication keys by ISSI.
    pub subscribers: Vec<SubscriberKey>,
    /// ISSIs whose `k` could not be parsed (reported at startup, never silently dropped).
    pub invalid_subscriber_keys: Vec<u32>,
}

/// Whether and how radios are authenticated when they register. With `Off` the cell trusts the
/// ISSI a radio claims; with `Optional` radios that have a key on file are challenged and the
/// rest let in; with `Required` a radio without a key, or one that fails, is rejected.
///   [security]
///   authentication = "required"
///   mutual_authentication = true
///   [[security.subscribers]]
///   issi = 2260571
///   k = "00112233445566778899aabbccddeeff"   # 128-bit K, as loaded into the radio
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthenticationMode {
    #[default]
    Off,
    Optional,
    Required,
}

impl AuthenticationMode {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Some(AuthenticationMode::Off),
            "optional" => Some(AuthenticationMode::Optional),
            "required" | "on" | "enforce" => Some(AuthenticationMode::Required),
            _ => None,
        }
    }
}

/// A subscriber's 128-bit authentication key K (EN 300 392-7 clause 4.1.5), keyed by ISSI.
#[derive(Clone, PartialEq, Eq)]
pub struct SubscriberKey {
    pub issi: u32,
    pub k: [u8; 16],
}

impl std::fmt::Debug for SubscriberKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SubscriberKey {{ issi: {}, k: <redacted> }}", self.issi)
    }
}

/// Parses `N` bytes of hex (spaces, `:` and `-` allowed as separators).
pub fn parse_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let digits: Vec<u8> = s
        .chars()
        .filter(|c| !matches!(c, ' ' | ':' | '-'))
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    if digits.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, pair) in digits.chunks(2).enumerate() {
        out[i] = (pair[0] << 4) | pair[1];
    }
    Some(out)
}

/// An 80-bit TETRA cipher key. `Debug` never prints the key, so a config dump or a log line
/// cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub struct CipherKey(pub [u8; 10]);

impl std::fmt::Debug for CipherKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CipherKey(<redacted>)")
    }
}

impl CipherKey {
    /// Parses 20 hex digits (spaces, `:` and `-` allowed as separators).
    pub fn from_hex(s: &str) -> Option<Self> {
        let digits: Vec<u8> = s
            .chars()
            .filter(|c| !matches!(c, ' ' | ':' | '-'))
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()?;
        if digits.len() != 20 {
            return None;
        }
        let mut key = [0u8; 10];
        for (i, pair) in digits.chunks(2).enumerate() {
            key[i] = (pair[0] << 4) | pair[1];
        }
        Some(CipherKey(key))
    }
}

/// Security class 2: one static cipher key shared by the infrastructure and every radio
/// (EN 300 392-7 clause 6.2). Radios without AIE still register in clear; the station never
/// bridges clear and encrypted radios (see Docs/aie-class2-plan.md §4a).
#[derive(Debug, Clone)]
pub struct CfgAie {
    /// `class = 2` in the config. With `class = 1` the key is still kept so it can be sent to
    /// radios over the air (OTAR) while the cell runs in clear; see `CfgSecurity::aie_staged`.
    pub enabled: bool,
    /// Key stream generator: 1 = TEA1 … 4 = TEA4.
    pub ksg: u8,
    /// Static cipher key number advertised in SYSINFO, 1..=32.
    pub sckn: u8,
    pub sck: CipherKey,
    /// SCK version number (16 bits, EN 300 392-7 Table A.102). Broadcast in SYSINFO in turn with
    /// the hyperframe number; its least significant bit is sent in every encrypted MAC-RESOURCE.
    /// Must match the version loaded into the radios with the key.
    pub sck_vn: u16,
    /// Talkgroups used by clear radios. Every other group is encrypted.
    pub clear_groups: Vec<u32>,
}

impl CfgAie {
    /// True when the group belongs to the clear (class 1) radios.
    pub fn is_clear_group(&self, gssi: u32) -> bool {
        self.clear_groups.contains(&gssi)
    }
}

impl Default for CfgSecurity {
    fn default() -> Self {
        CfgSecurity {
            issi_whitelist: Vec::new(),
            whitelist_mode: WhitelistMode::Auto,
            honour_unauthenticated_detach: true,
            max_registered_clients: DEFAULT_MAX_REGISTERED_CLIENTS,
            registration_rate_limit_per_min: DEFAULT_REGISTRATION_RATE_LIMIT_PER_MIN,
            aie: None,
            authentication: AuthenticationMode::Off,
            mutual_authentication: true,
            subscribers: Vec::new(),
            invalid_subscriber_keys: Vec::new(),
        }
    }
}

impl CfgSecurity {
    /// The authentication key K on file for an ISSI.
    pub fn subscriber_k(&self, issi: u32) -> Option<&[u8; 16]> {
        self.subscribers.iter().find(|s| s.issi == issi).map(|s| &s.k)
    }

    /// The SCK configured but not switched on (`class = 1` with a key): available for OTAR only.
    pub fn aie_staged(&self) -> Option<&CfgAie> {
        self.aie.as_ref().filter(|a| !a.enabled)
    }

    /// One-line description of the authentication posture, for the startup log.
    pub fn authentication_posture(&self) -> String {
        let n = self.subscribers.len();
        let bad = if self.invalid_subscriber_keys.is_empty() {
            String::new()
        } else {
            format!(" — IGNORED {} subscriber(s) with an invalid k: {:?}", self.invalid_subscriber_keys.len(), self.invalid_subscriber_keys)
        };
        let mutual = if self.mutual_authentication { "mutual" } else { "one-way" };
        match self.authentication {
            AuthenticationMode::Off => format!("OFF — radios are not authenticated ({n} key(s) configured but unused){bad}"),
            AuthenticationMode::Optional => format!("OPTIONAL — {n} radio(s) with a key are challenged ({mutual}), others register unauthenticated{bad}"),
            AuthenticationMode::Required => format!("REQUIRED — only the {n} radio(s) with a key may register ({mutual}){bad}"),
        }
    }

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
    #[serde(default)]
    pub aie: Option<CfgAieDto>,
    #[serde(default)]
    pub authentication: Option<String>,
    #[serde(default)]
    pub mutual_authentication: Option<bool>,
    #[serde(default)]
    pub subscribers: Vec<SubscriberKeyDto>,
}

#[derive(Clone, Default, Deserialize)]
pub struct SubscriberKeyDto {
    pub issi: u32,
    pub k: String,
}

impl std::fmt::Debug for SubscriberKeyDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SubscriberKeyDto {{ issi: {}, k: <redacted> }}", self.issi)
    }
}

#[derive(Clone, Default, Deserialize)]
pub struct CfgAieDto {
    /// 1 = clear (no AIE), 2 = static cipher key. Class 3 is not supported.
    #[serde(default)]
    pub class: Option<u8>,
    #[serde(default)]
    pub ksg: Option<u8>,
    #[serde(default)]
    pub sckn: Option<u8>,
    #[serde(default)]
    pub sck: Option<String>,
    #[serde(default)]
    pub sck_vn: Option<u16>,
    #[serde(default)]
    pub clear_groups: Vec<u32>,
}

impl std::fmt::Debug for CfgAieDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CfgAieDto")
            .field("class", &self.class)
            .field("ksg", &self.ksg)
            .field("sckn", &self.sckn)
            .field("sck_vn", &self.sck_vn)
            .field("clear_groups", &self.clear_groups)
            .finish_non_exhaustive()
    }
}

fn apply_aie_patch(dto: CfgAieDto) -> Result<Option<CfgAie>, String> {
    let enabled = match dto.class.unwrap_or(1) {
        // Class 1 without a key: plain clear cell. With a key: the key is staged for OTAR.
        1 if dto.sck.is_none() => return Ok(None),
        1 => false,
        2 => true,
        3 => return Err("security.aie: class 3 (authentication, DCK) is not supported; use class 2".into()),
        _ => return Err("security.aie.class must be 1 or 2".into()),
    };
    let ksg = dto.ksg.unwrap_or(1);
    if !(1..=4).contains(&ksg) {
        return Err("security.aie.ksg must be 1-4 (TEA1-TEA4)".into());
    }
    let sckn = dto.sckn.unwrap_or(1);
    if !(1..=32).contains(&sckn) {
        return Err("security.aie.sckn must be 1-32".into());
    }
    let Some(sck) = dto.sck.as_deref() else {
        return Err("security.aie.sck is required for class 2".into());
    };
    let sck = CipherKey::from_hex(sck).ok_or("security.aie.sck must be 20 hex digits (80 bits)")?;
    if dto.clear_groups.iter().any(|&g| g == 0 || g > 0xFF_FFFF) {
        return Err("security.aie.clear_groups: each GSSI must be 1-16777215".into());
    }
    Ok(Some(CfgAie {
        enabled,
        ksg,
        sckn,
        sck,
        sck_vn: dto.sck_vn.unwrap_or(0),
        clear_groups: dto.clear_groups,
    }))
}

pub fn apply_security_patch(dto: CfgSecurityDto) -> Result<CfgSecurity, String> {
    let defaults = CfgSecurity::default();
    // An unrecognised mode falls back to "auto"; the effective posture is logged at startup
    // (see access_control_posture) so a typo can't silently pass for a lockdown.
    let whitelist_mode = dto
        .whitelist_mode
        .as_deref()
        .map(|s| WhitelistMode::parse(s).unwrap_or(WhitelistMode::Auto))
        .unwrap_or(WhitelistMode::Auto);
    let mut subscribers = Vec::new();
    let mut invalid_subscriber_keys = Vec::new();
    for sub in dto.subscribers {
        match parse_hex::<16>(&sub.k) {
            Some(k) if sub.issi != 0 && sub.issi <= 0xFF_FFFF => subscribers.push(SubscriberKey { issi: sub.issi, k }),
            _ => invalid_subscriber_keys.push(sub.issi),
        }
    }
    let authentication = match dto.authentication.as_deref() {
        None => AuthenticationMode::Off,
        Some(s) => AuthenticationMode::parse(s).ok_or_else(|| format!("security.authentication = {s:?}: use off, optional or required"))?,
    };
    if authentication == AuthenticationMode::Required && subscribers.is_empty() {
        return Err("security.authentication = \"required\" needs at least one [[security.subscribers]] key, or no radio can register".into());
    }
    Ok(CfgSecurity {
        aie: apply_aie_patch(dto.aie.unwrap_or_default())?,
        authentication,
        mutual_authentication: dto.mutual_authentication.unwrap_or(defaults.mutual_authentication),
        subscribers,
        invalid_subscriber_keys,
        issi_whitelist: dto.issi_whitelist,
        whitelist_mode,
        honour_unauthenticated_detach: dto.honour_unauthenticated_detach.unwrap_or(defaults.honour_unauthenticated_detach),
        max_registered_clients: dto.max_registered_clients.unwrap_or(defaults.max_registered_clients),
        registration_rate_limit_per_min: dto
            .registration_rate_limit_per_min
            .unwrap_or(defaults.registration_rate_limit_per_min),
    })
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

    fn aie(class: u8, sck: Option<&str>) -> Result<Option<CfgAie>, String> {
        apply_aie_patch(CfgAieDto {
            class: Some(class),
            sck: sck.map(str::to_string),
            ..Default::default()
        })
    }

    #[test]
    fn aie_class2_parses_key_and_defaults() {
        let a = aie(2, Some("01 23 45 67 89 AB CD EF 01 23")).unwrap().unwrap();
        assert_eq!(a.sck.0, [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]);
        assert_eq!((a.ksg, a.sckn), (1, 1));
        assert!(!format!("{a:?}").contains("23"), "key must not appear in Debug output");
    }

    #[test]
    fn aie_rejects_bad_settings() {
        assert!(aie(1, None).unwrap().is_none(), "class 1 = clear");
        assert!(aie(2, None).is_err(), "class 2 needs a key");
        assert!(aie(2, Some("0123")).is_err(), "short key");
        assert!(aie(2, Some("0123456789ABCDEF012G")).is_err(), "non-hex key");
        assert!(aie(3, Some("0123456789ABCDEF0123")).is_err(), "class 3 unsupported");
    }

    #[test]
    fn authentication_and_staged_key_parse() {
        let dto: CfgSecurityDto = toml::from_str(
            "authentication = \"optional\"\n[[subscribers]]\nissi = 7\nk = \"00112233445566778899aabbccddeeff\"\n[[subscribers]]\nissi = 8\nk = \"zz\"\n[aie]\nclass = 1\nksg = 3\nsck = \"00112233445566778899\"\n",
        )
        .unwrap();
        let cfg = apply_security_patch(dto).unwrap();
        assert_eq!(cfg.authentication, AuthenticationMode::Optional);
        assert_eq!(cfg.subscribers.len(), 1);
        assert_eq!(cfg.invalid_subscriber_keys, vec![8]);
        assert!(cfg.subscriber_k(7).is_some());
        let staged = cfg.aie_staged().expect("class 1 with a key is staged");
        assert!(!staged.enabled);
        assert_eq!(staged.ksg, 3);
        assert!(cfg.authentication_posture().starts_with("OPTIONAL"));
        // required without keys is refused at load time
        let bad: CfgSecurityDto = toml::from_str("authentication = \"required\"\n").unwrap();
        assert!(apply_security_patch(bad).is_err());
    }
}
