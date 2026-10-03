/// Identifier of one TETRA cell (one SDR) inside a multi-cell station.
///
/// Cell 0 is always the primary cell described by the legacy `[cell_info]` + `[phy_io.soapysdr]`
/// sections; additional cells come from `[[cells]]` entries and use ids 1..=[`CellId::MAX`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CellId(pub u8);

impl CellId {
    /// The primary cell (legacy single-cell configuration).
    pub const PRIMARY: CellId = CellId(0);
    /// Highest allowed cell id. 8 cells in total keeps every sibling within the 7-entry
    /// D-NWRK-BROADCAST neighbour list.
    pub const MAX: u8 = 7;

    pub fn is_primary(&self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Display for CellId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cell{}", self.0)
    }
}
