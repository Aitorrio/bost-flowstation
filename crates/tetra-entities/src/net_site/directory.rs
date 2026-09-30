use std::collections::{BTreeSet, HashMap, HashSet};

use tetra_core::CellId;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};

/// Where radios are registered and which cells have members of each group.
#[derive(Debug, Default)]
pub struct SiteDirectory {
    /// ISSI → cell it last registered on.
    location: HashMap<u32, CellId>,
    /// (cell, ISSI) → groups that ISSI is affiliated to on that cell.
    affiliations: HashMap<(CellId, u32), HashSet<u32>>,
}

impl SiteDirectory {
    /// Apply a subscriber update a cell sent towards the network. Returns the cell the radio was
    /// registered on before, when this update is a registration that moved it here.
    pub fn apply(&mut self, cell: CellId, update: &MmSubscriberUpdate) -> Option<CellId> {
        let issi = update.issi;
        let mut moved_from = None;
        match update.action {
            BrewSubscriberAction::Register => {
                if let Some(old) = self.location.insert(issi, cell)
                    && old != cell
                {
                    // Moved: its groups on the old cell no longer have this ear.
                    self.affiliations.remove(&(old, issi));
                    moved_from = Some(old);
                }
                if !update.groups.is_empty() {
                    self.affiliations.entry((cell, issi)).or_default().extend(&update.groups);
                }
            }
            BrewSubscriberAction::Deregister => {
                if self.location.get(&issi) == Some(&cell) {
                    self.location.remove(&issi);
                }
                self.affiliations.remove(&(cell, issi));
            }
            BrewSubscriberAction::Affiliate => {
                self.location.entry(issi).or_insert(cell);
                self.affiliations.entry((cell, issi)).or_default().extend(&update.groups);
            }
            BrewSubscriberAction::Deaffiliate => {
                if let Some(groups) = self.affiliations.get_mut(&(cell, issi)) {
                    for g in &update.groups {
                        groups.remove(g);
                    }
                }
            }
        }
        moved_from
    }

    /// Cell the ISSI is registered on, if known.
    pub fn location(&self, issi: u32) -> Option<CellId> {
        self.location.get(&issi).copied()
    }

    /// Groups `issi` is affiliated to on `cell`.
    pub fn groups_of(&self, cell: CellId, issi: u32) -> Vec<u32> {
        let mut groups: Vec<u32> = self
            .affiliations
            .get(&(cell, issi))
            .map(|g| g.iter().copied().collect())
            .unwrap_or_default();
        groups.sort_unstable();
        groups
    }

    /// Cells with at least one member affiliated to `gssi`.
    pub fn group_cells(&self, gssi: u32) -> BTreeSet<CellId> {
        self.affiliations
            .iter()
            .filter(|(_, groups)| groups.contains(&gssi))
            .map(|((cell, _), _)| *cell)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(issi: u32, groups: &[u32], action: BrewSubscriberAction) -> MmSubscriberUpdate {
        MmSubscriberUpdate {
            issi,
            groups: groups.to_vec(),
            action,
        }
    }

    #[test]
    fn tracks_location_groups_and_moves() {
        let mut d = SiteDirectory::default();
        d.apply(CellId(0), &upd(100, &[], BrewSubscriberAction::Register));
        d.apply(CellId(0), &upd(100, &[91], BrewSubscriberAction::Affiliate));
        d.apply(CellId(1), &upd(200, &[91, 92], BrewSubscriberAction::Affiliate));
        assert_eq!(d.location(100), Some(CellId(0)));
        assert_eq!(d.location(200), Some(CellId(1)));
        assert_eq!(d.group_cells(91), BTreeSet::from([CellId(0), CellId(1)]));
        assert_eq!(d.group_cells(92), BTreeSet::from([CellId(1)]));

        // 100 re-registers on cell 1: cell 0 loses it as a group member.
        d.apply(CellId(1), &upd(100, &[], BrewSubscriberAction::Register));
        assert_eq!(d.location(100), Some(CellId(1)));
        d.apply(CellId(1), &upd(200, &[91], BrewSubscriberAction::Deaffiliate));
        assert!(d.group_cells(91).is_empty());

        // A late deregister from the old cell must not erase the new location.
        d.apply(CellId(0), &upd(100, &[], BrewSubscriberAction::Deregister));
        assert_eq!(d.location(100), Some(CellId(1)));
        d.apply(CellId(1), &upd(100, &[], BrewSubscriberAction::Deregister));
        assert_eq!(d.location(100), None);
    }
}
