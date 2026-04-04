#[cfg(feature = "server")]
use dashmap::DashMap;
use dioxus::prelude::*;
use dioxus_sdk_time::use_interval;
use serde::{Serialize, de::DeserializeOwned};
use std::env::home_dir;
#[cfg(feature = "server")]
use std::{
    fmt::Display,
    hash::Hash,
    sync::{Arc, OnceLock},
};
use time::{Duration, UtcDateTime};
#[cfg(feature = "server")]
use tokio::{
    fs,
    sync::{Mutex, MutexGuard, RwLock},
    time::{Duration as TokioDuration, sleep},
};

pub fn use_server_fn<F, T>(f: F, interval: time::Duration) -> Signal<Option<T>>
where
    F: AsyncFn() -> Result<T> + 'static + Copy,
    T: PartialEq + 'static,
{
    let mut state = use_signal(|| None);

    let body = move || async move {
        let new_state = f().await;

        if let Ok(new_state) = new_state
            && state.read().as_ref() != Some(&new_state)
        {
            state.set(Some(new_state));
        }
    };

    use_future(body);
    use_interval(
        std::time::Duration::from_nanos(interval.whole_nanoseconds() as u64),
        move |_| body(),
    );

    state
}

/// Set up statics and helper functions for caching.
#[macro_export]
macro_rules! caching {
    ($fn_name:ident, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[cfg(feature = "server")]
        static $const: crate::spotify::caching::SingleValueCache<$return> = crate::spotify::caching::SingleValueCache {
            in_mem_cache: tokio::sync::RwLock::const_new(None),
            last_fetched: tokio::sync::Mutex::const_new(UtcDateTime::MIN), // TODO: load last_fetched from cache
            interval: $interval,
            name: stringify!($fn_name),
        };

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }() -> $return {
            use crate::spotify::caching::Cache;
            $const.caching((), $closure).await
        }

        /// Client-Server function, returns Result for transport errors
        #[server]
        pub async fn $fn_name() -> Result<$return> {
            crate::assert_authenticated!();
            Ok(${ concat($fn_name, _server) }().await)
        }

        /// Client function, returns a Signal that updates every interval (
        #[doc = stringify!($interval)]
        /// )
        pub fn ${ concat(use_, $fn_name) }() -> Signal<Option<$return>> {
            use_server_fn($fn_name, $interval)
        }
    };
}

#[macro_export]
macro_rules! caching_hashmap {
    ($fn_name:ident, $key:ty, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[cfg(feature = "server")]
        static $const: crate::spotify::caching::HashmapCache<$key, $return> = crate::spotify::caching::HashmapCache {
            in_mem_cache: std::sync::OnceLock::new(),
            last_fetched: std::sync::LazyLock::new(dashmap::DashMap::new),
            interval: $interval,
            name: stringify!($fn_name),
        };

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }(key: $key) -> $return {
            use crate::spotify::caching::Cache;
            $const.caching(key, $closure).await
        }

        /// Client-Server function, returns Result for transport errors
        #[server]
        pub async fn $fn_name(key: $key) -> Result<$return> {
            crate::assert_authenticated!();
            Ok(${ concat($fn_name, _server) }(key).await)
        }

        // /// Client function, returns a Signal that updates every interval (
        // #[doc = stringify!($interval)]
        // /// )
        // pub fn ${ concat(use_, $fn_name) }() -> Signal<Option<$return>> {
        //     use_server_fn($fn_name, $interval)
        // }
    };
}

#[cfg(feature = "server")]
pub use server_only::*;

#[cfg(feature = "server")]
mod server_only {
    use dashmap::DashMap;
    use dashmap::mapref::one::Ref;
    use dioxus::prelude::*;
    use dioxus_sdk_time::use_interval;
    use serde::Deserialize;
    use serde::{Serialize, de::DeserializeOwned};
    use std::env::home_dir;
    use std::error::Error;
    use std::ops::{Deref, DerefMut};
    use std::path::PathBuf;
    use std::{
        fmt::Display,
        hash::Hash,
        sync::{Arc, LazyLock, OnceLock},
    };
    use time::{Duration, UtcDateTime};
    use tokio::sync::TryLockError;
    use tokio::{
        fs,
        sync::{Mutex, MutexGuard, RwLock},
        time::{Duration as TokioDuration, sleep},
    };

    macro_rules! traitSet {
        ($name:ident, $($traits:tt)+) => {
            trait $name: $($traits)+ {}
            impl<T> $name for T where T: $($traits)+ {}
        };
    }
    traitSet!(
        CacheKey,
        Hash + Eq + Serialize + DeserializeOwned + Clone + Send + Sync + 'static
    );
    traitSet!(
        CacheValue,
        Clone + Serialize + DeserializeOwned + Send + Sync + 'static
    );

    pub trait Cache: Sync {
        type Guard: Deref<Target = UtcDateTime> + DerefMut + Send;
        type K: CacheKey;
        type V: CacheValue;

        fn read_in_mem_cache(&self, key: &Self::K) -> impl Future<Output = Option<Self::V>> + Send;
        fn try_lock_last_fetched(&'static self, key: &Self::K)
        -> Result<Self::Guard, TryLockError>;
        fn interval(&self) -> Duration;
        fn write_mem_cache(&self, key: Self::K, value: Self::V) -> impl Future<Output = ()> + Send;
        fn disk_cache_path(&self, key: &Self::K) -> Option<PathBuf>;

        /// Return the result of `f`, caching to memory and disk, updating ever `interval`.
        /// While the value is being updated, stale values are handed out, without waiting for the update to finish.
        fn caching<F, Fut>(
            &'static self,
            key: Self::K,
            f: F,
        ) -> impl Future<Output = Self::V> + Send
        where
            F: Fn(Self::K, Option<Self::V>) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<Self::V, anyhow::Error>> + Send + 'static,
        {
            #[derive(Serialize, Deserialize)]
            struct WithLastFetched<V> {
                last_fetched: UtcDateTime,
                value: V,
            };

            async move {
                let now = UtcDateTime::now();

                let in_mem_cached = self.read_in_mem_cache(&key).await;
                let needs_update = self
                    .try_lock_last_fetched(&key)
                    .map(|last_fetched| (now > *last_fetched + self.interval(), last_fetched));
                match (in_mem_cached, needs_update) {
                    // there is a cached value, and it doesn't need updating
                    (Some(cached), Ok((false, _))) | (Some(cached), Err(_)) => cached.clone(),
                    // no other request is currently updating the value, and it needs updating; update it
                    (in_mem_cached, Ok((true, mut guard))) => {
                        let key_clone = key.clone();
                        let write_mem_and_disk_cache =
                            async move |value: Self::V, mut guard: Self::Guard| {
                                let path = self.disk_cache_path(&key_clone)?;
                                self.write_mem_cache(key_clone, value.clone()).await;

                                let now = UtcDateTime::now();
                                *guard = now;

                                let val = WithLastFetched {
                                    last_fetched: now,
                                    value,
                                };

                                fs::create_dir_all(path.parent()?).await.ok()?;
                                if let Err(e) =
                                    fs::write(path, serde_json::to_string(&val).ok()?).await
                                {
                                    warn!("Failed to write to cache: {e}")
                                }

                                drop(guard);
                                None::<()>
                            };

                        match in_mem_cached {
                            // fetch and write new value to cache in the background
                            Some(cached) => {
                                let clone = cached.clone();
                                tokio::spawn(async move {
                                    match f(key, Some(clone)).await {
                                        Ok(value) => {
                                            write_mem_and_disk_cache(value, guard).await;
                                        }
                                        Err(e) => error!("Failed to refresh value: {e}"),
                                    }
                                });

                                cached.clone()
                            }
                            None => {
                                let disk_cached = match self.disk_cache_path(&key) {
                                    Some(path) => match fs::read_to_string(path).await {
                                        Ok(contents) => {
                                            serde_json::from_str::<WithLastFetched<Self::V>>(
                                                &contents,
                                            )
                                            .ok()
                                        }
                                        Err(_) => None,
                                    },
                                    None => None,
                                };
                                match disk_cached {
                                    // return cached value, update last_fetched, update value on next fetch if necessary
                                    Some(WithLastFetched {
                                        last_fetched,
                                        value,
                                    }) => {
                                        self.write_mem_cache(key.clone(), value.clone()).await;
                                        *guard = last_fetched;

                                        value
                                    }
                                    // update synchronously, return new value once its available
                                    None => match f(key, None).await {
                                        Ok(value) => {
                                            write_mem_and_disk_cache(value.clone(), guard).await;
                                            value
                                        }
                                        Err(e) => panic!(
                                            "Failed to refresh value, and no cached one present: {e}"
                                        ),
                                    },
                                }
                            }
                        }
                    }
                    // cache not yet initialized, but another request is currently doing so; wait for that to complete
                    (None, Err(_)) => loop {
                        if let Some(value) = self.read_in_mem_cache(&key).await {
                            return value.clone();
                        }

                        sleep(TokioDuration::from_millis(100)).await;
                    },
                    (None, Ok((false, _))) => {
                        panic!("If the value doesn't need updating, there should be a cached value")
                    }
                }
            }
        }
    }

    pub struct SingleValueCache<V> {
        pub in_mem_cache: RwLock<Option<V>>,
        pub last_fetched: Mutex<UtcDateTime>,
        pub interval: Duration,
        pub name: &'static str,
    }
    impl<V> Cache for SingleValueCache<V>
    where
        V: CacheValue,
    {
        type Guard = tokio::sync::MutexGuard<'static, UtcDateTime>;
        type K = ();
        type V = V;

        fn interval(&self) -> Duration {
            self.interval
        }

        fn read_in_mem_cache(&self, _: &Self::K) -> impl Future<Output = Option<Self::V>> + Send {
            async move { self.in_mem_cache.read().await.clone() }
        }

        fn try_lock_last_fetched(&'static self, _: &Self::K) -> Result<Self::Guard, TryLockError> {
            self.last_fetched.try_lock()
        }

        fn write_mem_cache(&self, _: Self::K, value: Self::V) -> impl Future<Output = ()> + Send {
            async move { *self.in_mem_cache.write().await = Some(value) }
        }

        fn disk_cache_path(&self, _: &Self::K) -> Option<PathBuf> {
            home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{}.json", self.name));
                path
            })
        }
    }

    pub struct HashmapCache<K, V> {
        pub in_mem_cache: OnceLock<DashMap<K, V>>,
        pub last_fetched: LazyLock<DashMap<K, Arc<Mutex<UtcDateTime>>>>,
        pub interval: Duration,
        pub name: &'static str,
    }
    impl<K, V> Cache for HashmapCache<K, V>
    where
        K: CacheKey + Display,
        V: CacheValue,
    {
        type Guard = tokio::sync::OwnedMutexGuard<UtcDateTime>;
        type K = K;
        type V = V;

        fn interval(&self) -> Duration {
            self.interval
        }

        fn read_in_mem_cache(&self, key: &Self::K) -> impl Future<Output = Option<Self::V>> + Send {
            let value = self
                .in_mem_cache
                .get()
                .and_then(|map| map.get(key).map(|val| val.clone()));
            async move { value }
        }

        fn try_lock_last_fetched(&self, key: &Self::K) -> Result<Self::Guard, TryLockError> {
            let mutex = Arc::clone(
                &*self
                    .last_fetched
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(UtcDateTime::MIN))),
            );
            mutex.try_lock_owned()
        }
        fn write_mem_cache(&self, key: Self::K, value: Self::V) -> impl Future<Output = ()> + Send {
            let map = self.in_mem_cache.get_or_init(DashMap::new);
            async move {
                map.insert(key, value);
            }
        }

        fn disk_cache_path(&self, key: &Self::K) -> Option<PathBuf> {
            home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{}/{key}.json", self.name));
                path
            })
        }
    }
}
