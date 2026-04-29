#[cfg(feature = "server")]
use dashmap::DashMap;
use dioxus::prelude::*;
use dioxus_sdk_time::use_interval;
use serde::{Serialize, de::DeserializeOwned};
use std::any::Any;
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

#[typetag::serde]
pub trait Cacheable: Any + Send + Sync + 'static {}

/// Set up statics and helper functions for caching.
#[macro_export]
macro_rules! caching {
    ($fn_name:ident, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[typetag::serde]
        impl $crate::spotify::caching::Cacheable for $return {}

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }() -> $return {
            let wrapped_closure = |_, previous: Option<$return>| {
                async {
                    $closure(previous).await
                }
            };

            $crate::spotify::caching::caching(
                "",
                stringify!($fn_name),
                wrapped_closure,
                $interval
            ).await
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
        #[allow(unused)]
        pub fn ${ concat(use_, $fn_name) }() -> Signal<Option<$return>> {
            use_server_fn($fn_name, $interval)
        }
    };
}

#[macro_export]
macro_rules! caching_hashmap {
    ($fn_name:ident, $key:ty, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[typetag::serde]
        impl $crate::spotify::caching::Cacheable for $return {}

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }(key: $key) -> $return {
            $crate::spotify::caching::caching(
                key,
                stringify!($fn_name),
                $closure,
                $interval
            ).await
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
    use crate::spotify::caching::Cacheable;
    use dashmap::DashMap;
    use dashmap::mapref::one::Ref;
    use dioxus::prelude::*;
    use dioxus_sdk_time::use_interval;
    use foyer::{
        BlockEngineConfig, CacheEntry, Code, Compression, DeviceBuilder, EvictionConfig,
        FsDeviceBuilder, HybridCache, HybridCacheBuilder, HybridCachePolicy, HybridCacheProperties,
        SieveConfig,
    };
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use std::{
        any::Any,
        env::home_dir,
        error::Error,
        fmt::Display,
        hash::Hash,
        io,
        ops::{Deref, DerefMut},
        path::PathBuf,
        sync::{Arc, LazyLock, OnceLock},
    };
    use time::{Duration, UtcDateTime};
    use tokio::{
        fs,
        sync::{Mutex, MutexGuard, RwLock, TryLockError},
        time::{Duration as TokioDuration, sleep},
    };

    #[derive(Serialize, Deserialize)]
    struct CacheStruct {
        pub inner: Box<dyn Cacheable>,
        last_fetched: UtcDateTime,
    }

    pub static CACHE: OnceLock<HybridCache<String, CacheStruct>> = OnceLock::new();

    pub async fn setup_cache() -> foyer::Result<()> {
        let dir = home_dir()
            .ok_or(foyer::Error::io_error(io::Error::other(
                "Failed to get home dir",
            )))?
            .join(".cache/dash/");
        let device = FsDeviceBuilder::new(dir)
            .with_capacity(10 * 1024_usize.pow(3))
            .with_direct(true) // avoid double-caching from kernel
            .build()?;

        let cache = HybridCacheBuilder::new()
            .with_policy(HybridCachePolicy::WriteOnInsertion) // TODO: switch to eviction + shutdown handling with close()
            .memory(50 * 1024 * 1024)
            .with_eviction_config(EvictionConfig::Sieve(SieveConfig {}))
            .with_weighter(|key: &String, value: &CacheStruct| {
                key.estimated_size() + value.estimated_size()
            })
            .storage()
            .with_engine_config(BlockEngineConfig::new(device))
            .with_compression(Compression::Zstd)
            .build()
            .await?;

        CACHE.set(cache);

        Ok(())
    }

    pub fn caching<K, V, F, Fut>(
        key: K,
        key_prefix: &str,
        fetch: F,
        interval: Duration,
    ) -> impl Future<Output = V> + Send
    where
        K: Hash + Eq + Display + Send + Sync + Clone + 'static,
        V: Cacheable + Clone,
        F: Fn(K, Option<V>) -> Fut + Send + Clone + 'static,
        Fut: Future<Output = Result<V, anyhow::Error>> + Send + 'static,
    {
        async move {
            let now = UtcDateTime::now();

            let formatted_key = format!("{key_prefix}-{key}");

            let cache = CACHE.get().unwrap();
            let fetch = async |previous| {
                let key_clone = key.clone();
                cache
                    .get_or_fetch(
                        &formatted_key,
                        async move || -> anyhow::Result<CacheStruct> {
                            let inner: Box<dyn Cacheable> =
                                Box::new(fetch(key_clone, previous).await?);
                            Ok(CacheStruct {
                                inner,
                                last_fetched: now,
                            })
                        },
                    )
                    .await
                    .expect(&format!("Failed to get or fetch value for key {key}"))
            };

            let cached = fetch.clone()(None).await;

            let downcast =
                |wrapped: CacheEntry<String, CacheStruct, _, HybridCacheProperties>| -> V {
                    let any: &dyn Any = (&*wrapped.value().inner);
                    any.downcast_ref::<V>().unwrap().clone()
                };

            downcast(if now <= cached.value().last_fetched + interval {
                cached
            } else {
                cache.remove(&formatted_key);
                fetch(Some(downcast(cached))).await
            })
        }
    }
}
