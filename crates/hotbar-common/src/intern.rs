//! Path interning for zero-copy path lookups.
//!
//! Paths are interned once (at ingest or deserialization) and thereafter
//! referenced by a 4-byte `PathId` handle. Lookups in `HashMap<PathId, _>`
//! avoid per-comparison string hashing and heap-chasing.

use std::collections::HashMap;

/// Interned path handle — 4 bytes, Copy, trivially hashable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathId(u32);

/// Bidirectional path string <-> PathId table.
///
/// Owned by `HotState` on the daemon side. Panel-side code receives
/// `HotFile` with `path: String` over IPC; interning happens once at
/// the deserialization boundary.
pub struct PathInterner {
    to_id: HashMap<String, PathId>,
    to_str: Vec<String>,
}

impl PathInterner {
    /// Create an empty interner.
    pub fn new() -> Self {
        Self {
            to_id: HashMap::new(),
            to_str: Vec::new(),
        }
    }

    /// Intern a path string, returning a stable handle.
    ///
    /// If the path was already interned, returns the existing handle.
    /// Otherwise allocates a new slot.
    pub fn intern(&mut self, path: &str) -> PathId {
        if let Some(&id) = self.to_id.get(path) {
            return id;
        }
        let id = PathId(self.to_str.len() as u32);
        self.to_str.push(path.to_string());
        self.to_id.insert(path.to_string(), id);
        id
    }

    /// Resolve a handle back to the original path string.
    ///
    /// # Panics
    /// Panics if the `PathId` was not produced by this interner.
    pub fn resolve(&self, id: PathId) -> &str {
        &self.to_str[id.0 as usize]
    }

    /// Number of interned paths.
    pub fn len(&self) -> usize {
        self.to_str.len()
    }

    /// Whether the interner is empty.
    pub fn is_empty(&self) -> bool {
        self.to_str.is_empty()
    }
}

impl Default for PathInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_returns_same_id_for_same_path() {
        let mut interner = PathInterner::new();
        let a = interner.intern("/home/user/a.rs");
        let b = interner.intern("/home/user/a.rs");
        assert_eq!(a, b);
        assert_eq!(interner.len(), 1);
    }

    #[test]
    fn intern_returns_different_ids_for_different_paths() {
        let mut interner = PathInterner::new();
        let a = interner.intern("/home/user/a.rs");
        let b = interner.intern("/home/user/b.rs");
        assert_ne!(a, b);
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn resolve_roundtrips() {
        let mut interner = PathInterner::new();
        let id = interner.intern("/home/user/main.rs");
        assert_eq!(interner.resolve(id), "/home/user/main.rs");
    }

    #[test]
    fn path_id_is_copy_and_hashable() {
        let mut interner = PathInterner::new();
        let id = interner.intern("/test");
        let copy = id; // Copy
        assert_eq!(id, copy);

        // Usable as HashMap key
        let mut map: HashMap<PathId, usize> = HashMap::new();
        map.insert(id, 42);
        assert_eq!(map[&id], 42);
    }

    #[test]
    fn sequential_ids() {
        let mut interner = PathInterner::new();
        let a = interner.intern("/a");
        let b = interner.intern("/b");
        let c = interner.intern("/c");
        assert_eq!(a, PathId(0));
        assert_eq!(b, PathId(1));
        assert_eq!(c, PathId(2));
    }
}
