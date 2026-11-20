//! The view agora reopens with: sort order and, later, filters.

/// How the list is ordered under the live sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Newest first.
    #[default]
    Recent,
    /// By project name, newest first within each.
    Project,
    /// Largest transcript first.
    Size,
}

impl Sort {
    const ALL: [Sort; 3] = [Sort::Recent, Sort::Project, Sort::Size];

    pub fn next(self) -> Sort {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            Sort::Recent => "recent",
            Sort::Project => "project",
            Sort::Size => "size",
        }
    }

    #[allow(dead_code)] // read back by the saved view
    pub fn from_name(name: &str) -> Option<Sort> {
        Self::ALL.into_iter().find(|s| s.name() == name.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_cycle_and_round_trip_by_name() {
        assert_eq!(Sort::Recent.next(), Sort::Project);
        assert_eq!(Sort::Size.next(), Sort::Recent);
        for s in Sort::ALL {
            assert_eq!(Sort::from_name(s.name()), Some(s));
        }
        assert_eq!(Sort::from_name("nope"), None);
    }
}
