//! Typed identifiers. UUIDv7 so ids sort by creation time on disk.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(NotebookId);
id_type!(FolderId);
id_type!(NoteId);
id_type!(ImageId);
id_type!(TextId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_ids_sort_by_creation_time() {
        let a = NoteId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = NoteId::new();
        assert!(a < b, "uuid v7 ids must be time-ordered");
    }
}
