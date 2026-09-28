//! Point adapters: how a guest item becomes an extension point's item.
//!
//! An extension point's item is a Rust value (`CommandItem`, `PresetDescriptor`, an
//! `Arc<dyn Importer>`). A WASM plugin serves every item through one export, `call`; an
//! adapter wraps a [`GuestItem`] into the point's item type, so the registry, the loader
//! and every consumer see an ordinary item. The crate that defines a point, or the host
//! application, registers its adapter with [`crate::WasmHost::adapt`].

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

use forge_plugin::{ExtensionPoint, InstallCx, Order};

use crate::{GuestItem, WasmError};

/// Builds a point's item from a guest item.
pub type MakeFn<P> =
    Arc<dyn Fn(GuestItem) -> Result<<P as ExtensionPoint>::Item, WasmError> + Send + Sync>;
/// Wraps a point's item with a guest item (`chain`).
pub type WrapFn<P> = Arc<
    dyn Fn(GuestItem, <P as ExtensionPoint>::Item) -> <P as ExtensionPoint>::Item + Send + Sync,
>;

/// The operations a hosted plugin's install performs on one point.
pub(crate) trait PointAdapter: Send + Sync {
    fn add(&self, cx: &mut InstallCx, key: &str, item: GuestItem) -> Result<(), WasmError>;
    fn replace(&self, cx: &mut InstallCx, key: &str, item: GuestItem) -> Result<(), WasmError>;
    fn remove(&self, cx: &mut InstallCx, key: &str) -> Result<(), WasmError>;
    fn chain(&self, cx: &mut InstallCx, key: &str, item: GuestItem) -> Result<(), WasmError>;
}

/// Adapters by point manifest name.
pub(crate) type Adapters = BTreeMap<String, Arc<dyn PointAdapter>>;

pub(crate) struct Typed<P: ExtensionPoint> {
    make: MakeFn<P>,
    wrap: Option<WrapFn<P>>,
    _p: PhantomData<fn() -> P>,
}

impl<P: ExtensionPoint> Typed<P> {
    pub(crate) fn new(make: MakeFn<P>, wrap: Option<WrapFn<P>>) -> Self {
        Self {
            make,
            wrap,
            _p: PhantomData,
        }
    }
}

impl<P: ExtensionPoint> PointAdapter for Typed<P> {
    fn add(&self, cx: &mut InstallCx, key: &str, item: GuestItem) -> Result<(), WasmError> {
        let item = (self.make)(item)?;
        Ok(cx.add::<P>(key, item, Order::Last)?)
    }

    fn replace(&self, cx: &mut InstallCx, key: &str, item: GuestItem) -> Result<(), WasmError> {
        let item = (self.make)(item)?;
        Ok(cx.replace::<P>(key, item)?)
    }

    fn remove(&self, cx: &mut InstallCx, key: &str) -> Result<(), WasmError> {
        Ok(cx.remove::<P>(key)?)
    }

    fn chain(&self, cx: &mut InstallCx, key: &str, item: GuestItem) -> Result<(), WasmError> {
        let Some(wrap) = self.wrap.clone() else {
            return Err(WasmError::NoAdapter {
                plugin: item.plugin().to_string(),
                point: P::NAME.to_string(),
                op: "chains",
            });
        };
        Ok(cx.chain::<P>(key, move |inner| wrap(item, inner))?)
    }
}
