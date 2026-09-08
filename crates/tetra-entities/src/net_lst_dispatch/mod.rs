//! Local Site Trunking (LST) web dispatch — cell-local operator console.

mod entity;
mod handle;
mod media;
mod session;

pub use entity::LstDispatchEntity;
pub use handle::{LstDispatchHandle, LstUiCommand};
pub use media::codec_available;
pub use session::{ClaimResult, HEARTBEAT_HINT_SECS};

/// Sentinel brew-profile name stored in `active.json` / UI select value.
pub const LST_DISPATCH_PROFILE: &str = "__lst_dispatch__";

pub fn is_lst_profile_name(name: Option<&str>) -> bool {
    name == Some(LST_DISPATCH_PROFILE)
}
