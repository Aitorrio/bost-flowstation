//! A network link (telemetry, Fleet Control) that follows the configuration at run time. The
//! worker polls [`LiveLink::poll`] each loop; when its config section changed the link yields a
//! new transport (or `None` when the section was removed) and the worker reconnects.

use tetra_config::bluestation::{SharedConfig, StackConfig};

pub struct LiveLink<T> {
    cfg: SharedConfig,
    seen: u64,
    /// Hash of the last key: the key may hold passwords, so only its hash is kept.
    last: u64,
    key: fn(&StackConfig) -> String,
    build: Box<dyn Fn(&StackConfig) -> Option<T> + Send>,
}

fn digest(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

impl<T> LiveLink<T> {
    /// `key` renders the settings the link depends on (it is rebuilt only when this changes);
    /// `build` makes the transport for them, `None` when the link is not configured.
    pub fn new(cfg: SharedConfig, key: fn(&StackConfig) -> String, build: Box<dyn Fn(&StackConfig) -> Option<T> + Send>) -> Self {
        let seen = cfg.config_generation();
        let last = digest(&key(&cfg.config()));
        Self { cfg, seen, last, key, build }
    }

    /// `Some(change)` when the link's settings changed since the last call: `Some(transport)` to
    /// switch to, `None` to switch the link off.
    pub fn poll(&mut self) -> Option<Option<T>> {
        let generation = self.cfg.config_generation();
        if generation == self.seen {
            return None;
        }
        self.seen = generation;
        let config = self.cfg.config();
        let key = digest(&(self.key)(&config));
        if key == self.last {
            return None;
        }
        self.last = key;
        Some((self.build)(&config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::parsing;

    fn base() -> StackConfig {
        parsing::from_toml_str(include_str!("../../../example_config/config.toml")).unwrap()
    }

    #[test]
    fn rebuilds_only_when_its_section_changes() {
        let shared = SharedConfig::from_parts(base(), None);
        let mut link = LiveLink::new(shared.clone(), |c| format!("{:?}", c.service_name), Box::new(|c| Some(c.service_name.clone())));
        assert!(link.poll().is_none());
        let mut other = base();
        other.cell.location_area ^= 1;
        shared.replace_config(other).unwrap();
        assert!(link.poll().is_none(), "unrelated change: no rebuild");
        let mut named = base();
        named.service_name = Some("x".into());
        shared.replace_config(named).unwrap();
        assert_eq!(link.poll(), Some(Some(Some("x".to_string()))));
        assert!(link.poll().is_none());
    }

    #[test]
    fn a_changed_password_rebuilds_the_link() {
        let shared = SharedConfig::from_parts(base(), None);
        let mut link = LiveLink::new(
            shared.clone(),
            |c| tetra_config::bluestation::debug_with_secrets(&c.asterisk),
            Box::new(|c| Some(c.asterisk.password.val.clone())),
        );
        let mut other = base();
        other.asterisk.password = tetra_config::bluestation::SecretField::from("new-secret".to_string());
        shared.replace_config(other).unwrap();
        assert_eq!(link.poll(), Some(Some("new-secret".to_string())));
    }
}
