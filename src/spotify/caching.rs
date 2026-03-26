#[cfg(feature = "server")]
use dashmap::DashMap;
use dioxus::prelude::*;
use dioxus_sdk_time::use_interval;
use serde::{Serialize, de::DeserializeOwned};
use std::env::home_dir;
#[cfg(feature = "server")]
use std::{
    collections::HashMap,
    fmt::Display,
    hash::Hash,
    sync::{Arc, LazyLock, OnceLock},
};
use time::UtcDateTime;
#[cfg(feature = "server")]
use tokio::{
    fs,
    sync::{Mutex, MutexGuard, RwLock},
    time::{Duration as TokioDuration, sleep},
};

/// Return the result of `f`, caching to memory and disk, updating ever `interval`.
///
/// `last_fetched` serves as synchronization and interval control: whenever a request is updating the value, it locks the mutex, and updates the datetime inside.
/// The `in_mem_cache` and `last_fetched` are separated, to allow for quick cache retrieval even while the value is being updated.
/// `in_mem_cache` is only None before the first initialization.
#[cfg(feature = "server")]
pub async fn caching<T, F, Fut>(
    f: F,
    in_mem_cache: &'static RwLock<Option<T>>,
    last_fetched: &'static Mutex<UtcDateTime>,
    interval: time::Duration,
    name: &str,
) -> T
where
    T: Clone + Serialize + DeserializeOwned + Send + Sync,
    F: Fn(Option<T>) -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    let now = UtcDateTime::now();

    let read_mem_cache = async || in_mem_cache.read().await.clone();

    let in_mem_cached = read_mem_cache().await;
    let needs_update = last_fetched
        .try_lock()
        .map(|last_fetched| (now > *last_fetched + interval, last_fetched));
    match (in_mem_cached, needs_update) {
        // there is a cached value, and it doesn't need updating
        (Some(cached), Ok((false, _))) | (Some(cached), Err(_)) => cached,
        // no other request is currently updating the value, and it needs updating; update it
        (in_mem_cached, Ok((true, guard))) => {
            let write_mem_cache = async move |value: T| *in_mem_cache.write().await = Some(value);

            let disk_cache_path = home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{name}.json"));
                path
            });
            let disk_cache_path_clone = disk_cache_path.clone();
            let write_mem_and_disk_cache = async move |value: T| {
                write_mem_cache(value.clone()).await;
                let path = disk_cache_path_clone?;
                fs::create_dir_all(path.parent()?).await.ok()?;
                fs::write(path, serde_json::to_string(&value).ok()?)
                    .await
                    .ok()
            };

            let update_and_drop_last_fetched = move |mut guard: MutexGuard<_>| {
                *guard = UtcDateTime::now();
                drop(guard);
            };

            match in_mem_cached {
                // fetch and write new value to cache in the background
                Some(cached) => {
                    let clone = cached.clone();
                    tokio::spawn(async move {
                        write_mem_and_disk_cache(f(Some(clone)).await).await;

                        // hold lock until all caches are updated
                        update_and_drop_last_fetched(guard);
                    });

                    cached
                }
                None => {
                    let read_disk_cache = async || -> Option<T> {
                        serde_json::from_str(
                            &fs::read_to_string(disk_cache_path.clone()?).await.ok()?,
                        )
                        .ok()
                    };
                    match read_disk_cache().await {
                        // update asynchronously, return cached value
                        Some(cached) => {
                            write_mem_cache(cached.clone()).await;

                            let clone = cached.clone();
                            tokio::spawn(async move {
                                write_mem_and_disk_cache(f(Some(clone)).await).await;

                                // hold lock until all caches are updated
                                update_and_drop_last_fetched(guard);
                            });

                            cached
                        }
                        // update synchronously, return new value once its available
                        None => {
                            let new_value = f(None).await;
                            write_mem_and_disk_cache(new_value.clone()).await;

                            // hold lock until all caches are updated
                            update_and_drop_last_fetched(guard);

                            new_value
                        }
                    }
                }
            }
        }
        // cache not yet initialized, but another request is currently doing so; wait for that to complete
        (None, Err(_)) => loop {
            if let Some(value) = read_mem_cache().await {
                return value;
            }

            sleep(TokioDuration::from_millis(100)).await;
        },
        (None, Ok((false, _))) => {
            panic!("If the value doesn't need updating, there should be a cached value")
        }
    }
}

/// Set up statics and helper functions for caching.
#[macro_export]
macro_rules! caching {
    ($fn_name:ident, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[cfg(feature = "server")]
        static $const: RwLock<Option<$return>> = RwLock::const_new(None);
        #[cfg(feature = "server")]
        static ${ concat($const, _LAST_FETCH) }: Mutex<UtcDateTime> = Mutex::const_new(UtcDateTime::MIN); // initialize to min so the first access is always identified as after it

        /// Server-only function, returns output directly
        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }() -> $return {
            caching($closure, &$const, &${ concat($const, _LAST_FETCH) }, $interval, stringify!($fn_name)).await
        }

        /// Client-Server function, returns Result for transport errors
        #[server]
        pub async fn $fn_name() -> Result<$return> {
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

    use_future(move || body());
    use_interval(
        std::time::Duration::from_nanos(interval.whole_nanoseconds() as u64),
        move |_| body(),
    );

    state
}

#[cfg(feature = "server")]
pub async fn caching_hashmap<K, V, F, Fut>(
    f: F,
    key: K,
    in_mem_cache: &'static OnceLock<DashMap<K, V>>,
    last_fetched: &'static DashMap<K, Arc<Mutex<UtcDateTime>>>,
    interval: time::Duration,
    name: &str,
) -> V
where
    K: Hash + Eq + Serialize + DeserializeOwned + Clone + Send + Sync + Display,
    V: Clone + Serialize + DeserializeOwned + Send + Sync,
    F: Fn(Option<V>) -> Fut + Send + 'static,
    Fut: Future<Output = V> + Send + 'static,
{
    let now = UtcDateTime::now();

    let read_mem_cache = || -> Option<V> {
        in_mem_cache
            .get()
            .and_then(|map| map.get(&key).map(|val| val.clone()))
    };

    let in_mem_cached = read_mem_cache();
    let mutex = Arc::clone(
        &*last_fetched
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(UtcDateTime::MIN))),
    );
    let needs_update = mutex
        .try_lock_owned()
        .map(|last_fetched| (now > *last_fetched + interval, last_fetched));
    match (in_mem_cached, needs_update) {
        // there is a cached value, and it doesn't need updating
        (Some(cached), Ok((false, _))) | (Some(cached), Err(_)) => cached.clone(),
        // no other request is currently updating the value, and it needs updating; update it
        (in_mem_cached, Ok((true, guard))) => {
            use tokio::sync::OwnedMutexGuard;

            let key_clone = key.clone();
            let write_mem_cache = move |value: V| {
                let map = in_mem_cache.get_or_init(DashMap::new);
                map.insert(key_clone, value);
            };

            let disk_cache_path = home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{name}/{key}.json"));
                path
            });
            let disk_cache_path_clone = disk_cache_path.clone();
            let write_mem_cache_clone = write_mem_cache.clone();
            let write_mem_and_disk_cache = async move |value: V| {
                write_mem_cache_clone(value.clone());
                let path = disk_cache_path_clone?;
                fs::create_dir_all(path.parent()?).await.ok()?;
                fs::write(path, serde_json::to_string(&value).ok()?)
                    .await
                    .ok()
            };

            let update_and_drop_last_fetched = move |mut guard: OwnedMutexGuard<_>| {
                *guard = UtcDateTime::now();
                drop(guard);
            };

            match in_mem_cached {
                // fetch and write new value to cache in the background
                Some(cached) => {
                    let clone = cached.clone();
                    tokio::spawn(async move {
                        write_mem_and_disk_cache(f(Some(clone)).await).await;

                        // hold lock until all caches are updated
                        update_and_drop_last_fetched(guard);
                    });

                    cached.clone()
                }
                None => {
                    let read_disk_cache = async || -> Option<V> {
                        serde_json::from_str::<V>(
                            &fs::read_to_string(disk_cache_path.clone()?).await.ok()?,
                        )
                        .ok()
                    };
                    match read_disk_cache().await {
                        // update asynchronously, return cached value
                        Some(cached) => {
                            write_mem_cache(cached.clone());

                            let clone = cached.clone();
                            tokio::spawn(async move {
                                write_mem_and_disk_cache(f(Some(clone)).await).await;

                                // hold lock until all caches are updated
                                update_and_drop_last_fetched(guard);
                            });

                            cached.clone()
                        }
                        // update synchronously, return new value once its available
                        None => {
                            let new_value = f(None).await;
                            write_mem_and_disk_cache(new_value.clone()).await;

                            // hold lock until all caches are updated
                            update_and_drop_last_fetched(guard);

                            new_value
                        }
                    }
                }
            }
        }
        // cache not yet initialized, but another request is currently doing so; wait for that to complete
        (None, Err(_)) => loop {
            if let Some(value) = read_mem_cache() {
                return value.clone();
            }

            sleep(TokioDuration::from_millis(100)).await;
        },
        (None, Ok((false, _))) => {
            panic!("If the value doesn't need updating, there should be a cached value")
        }
    }
}
