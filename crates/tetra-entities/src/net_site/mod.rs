//! Site switch: makes the cells of a multi-cell station look like one stack to the network
//! entity (Brew or LST Dispatch).
//!
//! Every cell's radio stack runs on its own thread with its own router. The network entity lives
//! only in the primary router, wrapped by [`SiteSwitch`] in the `TetraEntity::Brew` slot. Each
//! additional cell has a [`CellLink`] in its Brew slot that forwards everything to the switch.
//!
//! The switch:
//! * tracks where radios are registered and which cells have members of each group, from the
//!   `MmSubscriberUpdate`s the cells send to the network;
//! * routes network → stack messages to the right cell: by carrier (carriers are unique per
//!   cell), by Brew session UUID, by destination ISSI/GSSI location, else to the primary;
//! * fans a network group call out to every cell with members, forwards only the first
//!   `NetworkCallReady` to the network entity and copies its downlink voice to the other cells;
//! * delivers SDS between cells directly instead of via the network;
//! * renumbers call identifiers, which each cell's CMCE allocates independently, so they are
//!   unique towards the network entity.
//!
//! Not yet (multi-cell plan, phase 4): a call started by a radio on one cell is not heard on the
//! other cells, and individual calls between radios on different cells are not connected.

mod directory;
mod switch;

pub use directory::SiteDirectory;
pub use switch::{CellLink, SitePorts, SiteSwitch, site_links};
