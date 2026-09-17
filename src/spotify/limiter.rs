use dioxus::prelude::trace;
use rspotify::AuthCodeSpotify;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore},
    time::{Instant, sleep_until},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    MyPlaylists,
    Playlists,
    SavedTracks,
    Player,
    Tracks,
    Artists,
    Me,
    LastFmTopTags,
}

impl Endpoint {
    const ALL: [Self; 8] = [
        Self::MyPlaylists,
        Self::Playlists,
        Self::SavedTracks,
        Self::Player,
        Self::Tracks,
        Self::Artists,
        Self::Me,
        Self::LastFmTopTags,
    ];

    const fn index(self) -> usize {
        match self {
            Self::MyPlaylists => 0,
            Self::Playlists => 1,
            Self::SavedTracks => 2,
            Self::Player => 3,
            Self::Tracks => 4,
            Self::Artists => 5,
            Self::Me => 6,
            Self::LastFmTopTags => 7,
        }
    }
}

struct EndpointPool {
    accounts: Vec<AccountSlot>,
    state: Mutex<EndpointState>,
    notify: Notify,
}

struct AccountSlot {
    request: Arc<Semaphore>,
}

struct EndpointState {
    next_account: usize,
    unavailable_until: Vec<Option<Instant>>,
}

pub struct SpotifyApi {
    clients: Vec<Arc<AuthCodeSpotify>>,
    endpoints: [Arc<EndpointPool>; Endpoint::ALL.len()],
}

impl SpotifyApi {
    pub fn new(clients: Vec<Arc<AuthCodeSpotify>>) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(
            !clients.is_empty(),
            "at least one Spotify account must be configured"
        );

        let endpoints = Endpoint::ALL.map(|_| {
            Arc::new(EndpointPool {
                accounts: (0..clients.len())
                    .map(|_| AccountSlot {
                        request: Arc::new(Semaphore::new(1)),
                    })
                    .collect(),
                state: Mutex::new(EndpointState {
                    next_account: 0,
                    unavailable_until: vec![None; clients.len()],
                }),
                notify: Notify::new(),
            })
        });

        Ok(Arc::new(Self { clients, endpoints }))
    }

    pub fn client(&self, account: usize) -> Arc<AuthCodeSpotify> {
        Arc::clone(&self.clients[account])
    }

    pub async fn acquire(self: &Arc<Self>, endpoint: Endpoint) -> SpotifyLease {
        let pool = Arc::clone(&self.endpoints[endpoint.index()]);

        loop {
            // Create the notification future before inspecting the state. This
            // avoids missing a release between checking and waiting.
            let notified = pool.notify.notified();

            if let Some((account, permit)) = Self::try_acquire(&pool).await {
                drop(notified);
                return SpotifyLease {
                    api: Arc::clone(self),
                    pool,
                    endpoint,
                    account,
                    permit: Some(permit),
                };
            }

            match Self::next_wakeup(&pool).await {
                Some(until) => {
                    tokio::select! {
                        _ = notified => {}
                        _ = sleep_until(until) => {}
                    }
                }
                None => notified.await,
            }
        }
    }

    async fn try_acquire(pool: &EndpointPool) -> Option<(usize, OwnedSemaphorePermit)> {
        let now = Instant::now();
        let mut state = pool.state.lock().await;

        for offset in 0..pool.accounts.len() {
            let account = (state.next_account + offset) % pool.accounts.len();

            if state.unavailable_until[account].is_some_and(|until| until > now) {
                continue;
            }
            state.unavailable_until[account] = None;

            if let Ok(permit) = Arc::clone(&pool.accounts[account].request).try_acquire_owned() {
                state.next_account = (account + 1) % pool.accounts.len();
                return Some((account, permit));
            }
        }

        None
    }

    async fn next_wakeup(pool: &EndpointPool) -> Option<Instant> {
        let now = Instant::now();
        let mut state = pool.state.lock().await;
        state
            .unavailable_until
            .iter_mut()
            .filter_map(|until| {
                if until.is_some_and(|value| value <= now) {
                    *until = None;
                }
                *until
            })
            .min()
    }
}

pub struct SpotifyLease {
    api: Arc<SpotifyApi>,
    pool: Arc<EndpointPool>,
    endpoint: Endpoint,
    account: usize,
    permit: Option<OwnedSemaphorePermit>,
}

impl SpotifyLease {
    pub fn client(&self) -> Arc<AuthCodeSpotify> {
        self.api.client(self.account)
    }

    pub async fn rate_limited(mut self, retry_after: Duration) {
        let unavailable_until = Instant::now() + retry_after;
        let mut state = self.pool.state.lock().await;
        state.unavailable_until[self.account] = Some(unavailable_until);
        drop(state);
        self.permit.take();
        self.pool.notify.notify_waiters();
    }
}

impl Drop for SpotifyLease {
    fn drop(&mut self) {
        self.permit.take();
        self.pool.notify.notify_waiters();
        trace!(
            "Released Spotify account {} for {:?}",
            self.account, self.endpoint
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(account_count: usize) -> Arc<SpotifyApi> {
        SpotifyApi::new(
            (0..account_count)
                .map(|_| Arc::new(AuthCodeSpotify::default()))
                .collect(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn leases_are_round_robin_per_endpoint() {
        let api = api(2);
        let first = api.acquire(Endpoint::Tracks).await;
        assert_eq!(first.account(), 0);
        drop(first);

        let second = api.acquire(Endpoint::Tracks).await;
        assert_eq!(second.account(), 1);
        drop(second);

        let third = api.acquire(Endpoint::Tracks).await;
        assert_eq!(third.account(), 0);
    }

    #[tokio::test]
    async fn cooldown_skips_account_and_release_wakes_waiter() {
        let api = api(2);
        let first = api.acquire(Endpoint::Tracks).await;
        first.rate_limited(Duration::from_millis(100)).await;

        let second = api.acquire(Endpoint::Tracks).await;
        assert_eq!(second.account(), 1);

        let waiting_api = Arc::clone(&api);
        let waiting =
            tokio::spawn(async move { waiting_api.acquire(Endpoint::Tracks).await.account() });

        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(second);

        assert_eq!(waiting.await.unwrap(), 1);
    }
}
