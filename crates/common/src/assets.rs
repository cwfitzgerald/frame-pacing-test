use std::{
    any::{Any, TypeId},
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use anyhow::Context;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;

pub fn load_file(path: &str) -> anyhow::Result<Vec<u8>> {
    let _span = tracy_client::span!("load_file");
    std::fs::read(format!("{}/../../assets/{path}", env!("CARGO_MANIFEST_DIR")))
        .with_context(|| format!("Failed to read file: {path}"))
}

pub struct Asset<T>(Arc<OnceCell<T>>);

impl<T> Clone for Asset<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Asset<T>
where
    T: Send + Sync + 'static,
{
    fn new() -> Self {
        Self(Arc::new(OnceCell::new()))
    }

    pub fn uncached_asset(data: T) -> Self {
        let asset = Self::new();
        asset.set(data);
        asset
    }

    fn to_erased(&self) -> ErasedAsset {
        ErasedAsset(self.0.clone())
    }

    fn from_erased(erased: ErasedAsset) -> Self {
        Self(erased.0.downcast::<OnceCell<T>>().unwrap())
    }

    fn into_future(self) -> FutureAsset<T> {
        FutureAsset(self.0.clone())
    }

    fn set(&self, data: T) {
        self.0.set(data).map_err(|_| "Failed to set asset data").unwrap();
    }

    fn wait_for_completion(&self) {
        let _span = tracy_client::span!("Asset::wait_for_completion");
        self.0.wait();
    }
}

impl<T> std::ops::Deref for Asset<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.get().unwrap()
    }
}

#[derive(Clone)]
struct ErasedAsset(Arc<dyn Any + Send + Sync>);

pub struct FutureAsset<T>(Arc<OnceCell<T>>);

impl<T> FutureAsset<T>
where
    T: Send + Sync + 'static,
{
    pub fn try_get(&self) -> Option<Asset<T>> {
        if self.0.get().is_some() {
            Some(Asset(self.0.clone()))
        } else {
            None
        }
    }

    pub fn wait(self) -> Asset<T> {
        let asset = Asset(self.0.clone());

        asset.wait_for_completion();

        asset
    }
}

#[derive(Clone)]
pub struct AssetCache {
    // AssetItem behind the dyn Any
    inner: Arc<Mutex<HashMap<(String, TypeId), ErasedAsset>>>,
}

impl AssetCache {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn load<T, F>(&self, path: &str, f: F) -> FutureAsset<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + Sync + 'static,
    {
        let mut inner = self.inner.lock();

        match inner.entry((path.to_string(), TypeId::of::<T>())) {
            Entry::Occupied(entry) => {
                let arc = entry.get().clone();
                let asset = Asset::<T>::from_erased(arc);
                drop(inner);

                asset.into_future()
            }
            Entry::Vacant(v) => {
                let asset = Asset::<T>::new();
                let arc = asset.to_erased();
                v.insert(arc);
                drop(inner);

                let path_clone = path.to_string();
                let asset_clone = asset.clone();
                rayon::spawn(move || {
                    let _span = tracy_client::span!("AssetCache::load");
                    log::info!(
                        "Loading asset type {ty:?} (rayon): {path_clone}",
                        ty = std::any::type_name::<T>()
                    );
                    asset_clone.set(f());
                });

                asset.into_future()
            }
        }
    }

    pub fn load_blocking<T, F>(&self, path: &str, f: F) -> Asset<T>
    where
        F: FnOnce() -> T,
        T: Send + Sync + 'static,
    {
        let _span = tracy_client::span!("AssetCache::load_blocking");

        let mut inner = self.inner.lock();

        match inner.entry((path.to_string(), TypeId::of::<T>())) {
            Entry::Occupied(entry) => {
                let arc = entry.get().clone();
                let asset = Asset::<T>::from_erased(arc);
                drop(inner);

                asset.wait_for_completion();

                asset
            }
            Entry::Vacant(v) => {
                let asset = Asset::<T>::new();
                let arc = asset.to_erased();
                v.insert(arc);
                drop(inner);

                let _span = tracy_client::span!("Calling callback");
                log::info!("Loading asset {ty} (sync): {path}", ty = std::any::type_name::<T>());
                asset.set(f());

                asset
            }
        }
    }
}

impl Default for AssetCache {
    fn default() -> Self {
        Self::new()
    }
}
