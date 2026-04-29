use dioxus::prelude::*;
use dioxus_sdk_time::use_interval;

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
        #[allow(clippy::crate_in_macro_def)]
        #[cfg(feature = "server")]
        static $const: crate::spotify::caching::SingleValueCache<$return> = crate::spotify::caching::SingleValueCache {
            last_fetched: tokio::sync::Mutex::const_new(UtcDateTime::MIN), // TODO: load last_fetched from cache
            interval: $interval,
            name: stringify!($fn_name),
            _v: std::marker::PhantomData,
        };

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }() -> $return {
            #[allow(clippy::crate_in_macro_def)]
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
        #[allow(unused)]
        pub fn ${ concat(use_, $fn_name) }() -> Signal<Option<$return>> {
            use_server_fn($fn_name, $interval)
        }
    };
}

#[macro_export]
macro_rules! caching_hashmap {
    ($fn_name:ident, $key:ty, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[allow(clippy::crate_in_macro_def)]
        #[cfg(feature = "server")]
        static $const: crate::spotify::caching::HashmapCache<$key, $return> = crate::spotify::caching::HashmapCache {
            last_fetched: std::sync::LazyLock::new(dashmap::DashMap::new),
            interval: $interval,
            name: stringify!($fn_name),
            _v: std::marker::PhantomData,
        };

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }(key: $key) -> $return {
            #[allow(clippy::crate_in_macro_def)]
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
    use dioxus::prelude::*;
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use std::{
        env::home_dir,
        fmt::Display,
        hash::Hash,
        marker::PhantomData,
        ops::{Deref, DerefMut},
        path::PathBuf,
        sync::{Arc, LazyLock},
    };
    use time::{Duration, UtcDateTime};
    use tokio::{
        fs,
        sync::{Mutex, TryLockError},
        time::{Duration as TokioDuration, sleep},
    };

    #[derive(Serialize, Deserialize)]
    pub struct WithLastFetched<V> {
        pub last_fetched: UtcDateTime,
        pub value: V,
    }
    impl<V> Default for WithLastFetched<V>
    where
        V: Default,
    {
        fn default() -> Self {
            Self {
                last_fetched: UtcDateTime::MIN,
                value: V::default(),
            }
        }
    }

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

        fn try_lock_last_fetched(&'static self, key: &Self::K)
        -> Result<Self::Guard, TryLockError>;
        fn interval(&self) -> Duration;
        fn disk_cache_path(&self, key: &Self::K) -> Option<PathBuf>;

        fn read_disk_cache(
            &self,
            key: &Self::K,
        ) -> impl Future<Output = Option<WithLastFetched<Self::V>>> + Send {
            async {
                match self.disk_cache_path(key) {
                    Some(path) => fs::read_to_string(path).await.ok().and_then(|contents| {
                        serde_json::from_str::<WithLastFetched<Self::V>>(&contents).ok()
                    }),
                    None => None,
                }
            }
        }

        fn write_disk_cache(
            &self,
            key: &Self::K,
            value: WithLastFetched<Self::V>,
        ) -> impl Future<Output = anyhow::Result<()>> + Send {
            async move {
                let path = self
                    .disk_cache_path(key)
                    .context("Failed to get disk cache path")?;
                fs::create_dir_all(path.parent().context("Disk cache path has no parent")?)
                    .await
                    .context("Failed to create disk cache parent dir")?;
                fs::write(
                    path,
                    serde_json::to_string(&value).context("Failed to serialize value")?,
                )
                .await
                .context("Failed to write to disk cache")
            }
        }

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
            async move {
                let now = UtcDateTime::now();

                let cached = self.read_disk_cache(&key).await;
                let needs_update = self
                    .try_lock_last_fetched(&key)
                    .map(|last_fetched| (now > *last_fetched + self.interval(), last_fetched));
                match (cached, needs_update) {
                    // there is a cached value, and it doesn't need updating
                    (Some(cached), Ok((false, _))) | (Some(cached), Err(_)) => cached.value.clone(),
                    // no other request is currently updating the value, and it needs updating; update it
                    (cached, Ok((true, guard))) => {
                        let key_clone = key.clone();
                        let write_disk_cache =
                            async move |value: Self::V, mut guard: Self::Guard| {
                                let now = UtcDateTime::now();
                                *guard = now;

                                let value = WithLastFetched {
                                    last_fetched: now,
                                    value,
                                };

                                self.write_disk_cache(&key_clone, value).await

                                // guard dropped
                            };

                        match cached {
                            // fetch and write new value to cache in the background
                            Some(WithLastFetched { value, .. }) => {
                                let clone = value.clone();
                                tokio::spawn(async move {
                                    match f(key, Some(clone)).await {
                                        Ok(value) => {
                                            if let Err(e) = write_disk_cache(value, guard).await {
                                                error!("Failed to write to disk cache: {e}")
                                            }
                                        }
                                        Err(e) => error!("Failed to refresh value: {e}"),
                                    }
                                });

                                value.clone()
                            }
                            // update synchronously, return new value once its available
                            None => match f(key, None).await {
                                Ok(value) => {
                                    if let Err(e) = write_disk_cache(value.clone(), guard).await {
                                        error!("Failed to write to disk cache: {e}")
                                    }

                                    value
                                }
                                Err(e) => panic!(
                                    "Failed to refresh value, and no cached one present: {e}"
                                ),
                            },
                        }
                    }
                    // cache not yet initialized, but another request is currently doing so; wait for that to complete
                    (None, Err(_)) => loop {
                        if let Some(value) = self.read_disk_cache(&key).await {
                            return value.value.clone();
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
        pub last_fetched: Mutex<UtcDateTime>,
        pub interval: Duration,
        pub name: &'static str,
        pub _v: PhantomData<V>,
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
        fn try_lock_last_fetched(&'static self, _: &Self::K) -> Result<Self::Guard, TryLockError> {
            self.last_fetched.try_lock()
        }
        fn disk_cache_path(&self, _: &Self::K) -> Option<PathBuf> {
            home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{}.json", self.name));
                path
            })
        }
    }

    pub struct HashmapCache<K, V> {
        pub last_fetched: LazyLock<DashMap<K, Arc<Mutex<UtcDateTime>>>>,
        pub interval: Duration,
        pub name: &'static str,
        pub _v: PhantomData<V>,
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

        fn try_lock_last_fetched(&self, key: &Self::K) -> Result<Self::Guard, TryLockError> {
            let mutex = Arc::clone(
                &*self
                    .last_fetched
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(UtcDateTime::MIN))),
            );
            mutex.try_lock_owned()
        }

        fn disk_cache_path(&self, key: &Self::K) -> Option<PathBuf> {
            home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{}/{key}.json", self.name));
                path
            })
        }
    }
}
