//! [`CatalogIndex`]: forge-ui's [`AssetIndex`] (what the asset reference picker and the
//! property grid's asset rows read) over the editor's asset catalogue — the real asset
//! database (Ch.8). forge-ui sits beside forge-asset and may not depend on it, so the
//! adapter lives here. It reads the catalogue live: an import, rename or removal (by anyone,
//! through the bus) is what the picker offers next time it asks.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::assets::AssetCatalog;
use forge_ui::widgets::{AssetEntry, AssetIndex};

/// See the module docs.
pub struct CatalogIndex {
    catalog: Rc<RefCell<dyn AssetCatalog>>,
}

impl CatalogIndex {
    pub fn new(catalog: Rc<RefCell<dyn AssetCatalog>>) -> Self {
        Self { catalog }
    }
}

impl AssetIndex for CatalogIndex {
    fn get(&self, path: &str) -> Option<AssetEntry> {
        self.catalog
            .borrow()
            .rows()
            .into_iter()
            .find(|r| r.path == path)
            .map(|r| AssetEntry {
                path: r.path,
                kind: r.kind,
            })
    }
    /// Kinds compare case-insensitively (the picker says `Texture`, the database `texture`).
    fn of_kind(&self, kind: &str) -> Vec<AssetEntry> {
        self.catalog
            .borrow()
            .rows()
            .into_iter()
            .filter(|r| r.kind.eq_ignore_ascii_case(kind))
            .map(|r| AssetEntry {
                path: r.path,
                kind: r.kind,
            })
            .collect()
    }
}
