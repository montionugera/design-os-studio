//! Helper for loading enabled `LibrarySnapshot`s for MCP spec tools (`E15-1`).

use designos_doc::{Document, LibrarySnapshot};
use designos_library::Library;
use std::path::Path;

/// Loads the published `LibrarySnapshot` for each library enabled in `doc.library.enabled`
/// from the default `designos_library::Library::default_root()`.
pub fn load_enabled_snapshots(doc: &Document) -> Vec<LibrarySnapshot> {
    if doc.library.enabled.is_empty() {
        return Vec::new();
    }
    load_enabled_snapshots_from(doc, &Library::default_root())
}

/// Loads the published `LibrarySnapshot` for each library enabled in `doc.library.enabled`
/// from `root`.
pub fn load_enabled_snapshots_from(doc: &Document, root: &Path) -> Vec<LibrarySnapshot> {
    if doc.library.enabled.is_empty() || !root.exists() {
        return Vec::new();
    }
    let Ok(lib) = Library::open(root) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(doc.library.enabled.len());
    for lib_ref in &doc.library.enabled {
        let snap = lib.load_snapshot(&lib_ref.id, None).or_else(|_| lib.load_snapshot(&lib_ref.id, (lib_ref.version > 0).then_some(lib_ref.version)));
        if let Ok(snap) = snap {
            out.push(snap);
        }
    }
    out
}
