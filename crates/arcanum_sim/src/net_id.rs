//! Host-issued entity identifier, stable across the wire.
//!
//! Every entity the host creates gets a [`NetId`]. Clients refer to entities by `NetId`, never
//! by their local Bevy `Entity`. The host assigns them from a monotonic counter.

use serde::{Deserialize, Serialize};

/// A host-issued entity tag. Two `NetId`s from the same session are equal iff they name the
/// same entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetId(pub u64);

/// Hands out [`NetId`]s. The host owns exactly one of these.
#[derive(Debug, Default)]
pub struct NetIdAllocator {
    next: u64,
}

impl NetIdAllocator {
    pub fn allocate(&mut self) -> NetId {
        let id = NetId(self.next);
        self.next += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_sequential() {
        let mut alloc = NetIdAllocator::default();
        let a = alloc.allocate();
        let b = alloc.allocate();
        assert_ne!(a, b);
        assert_eq!(a.0 + 1, b.0);
    }

    #[test]
    fn net_id_serialises_round_trip() {
        let id = NetId(42);
        let json = serde_json::to_string(&id).unwrap();
        let back: NetId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
