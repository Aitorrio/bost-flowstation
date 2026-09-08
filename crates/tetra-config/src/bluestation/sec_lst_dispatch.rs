use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// Local Site Trunking (LST) web dispatch console — cell-local operator, never Brew.
#[derive(Debug, Clone)]
pub struct CfgLstDispatch {
    /// When true, start LstDispatchEntity (requires brew == None).
    pub enabled: bool,
    /// Synthetic operator ISSI shown on the air as the dispatcher.
    pub operator_issi: u32,
    /// Optional default GSSI to preselect in the UI (0 = none).
    pub default_gssi: u32,
}

impl Default for CfgLstDispatch {
    fn default() -> Self {
        Self {
            enabled: false,
            operator_issi: 9_990_001,
            default_gssi: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfgLstDispatchDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_operator_issi")]
    pub operator_issi: u32,
    #[serde(default)]
    pub default_gssi: u32,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgLstDispatchDto {
    fn default() -> Self {
        Self {
            enabled: false,
            operator_issi: default_operator_issi(),
            default_gssi: 0,
            extra: HashMap::new(),
        }
    }
}

fn default_operator_issi() -> u32 {
    9_990_001
}

pub fn apply_lst_dispatch_patch(dto: CfgLstDispatchDto) -> CfgLstDispatch {
    CfgLstDispatch {
        enabled: dto.enabled,
        operator_issi: dto.operator_issi.clamp(1, 16_777_214),
        default_gssi: if dto.default_gssi == 0 {
            0
        } else {
            dto.default_gssi.clamp(1, 16_777_214)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = apply_lst_dispatch_patch(CfgLstDispatchDto::default());
        assert!(!c.enabled);
        assert_eq!(c.operator_issi, 9_990_001);
    }
}
