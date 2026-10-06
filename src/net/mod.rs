
pub mod artist;
pub mod browse;
pub mod cache;
pub mod covers;
pub mod explore;
pub mod history;
pub mod home;
pub mod items;
pub mod local_feed;
pub mod library;
pub mod links;
pub mod online;
pub mod player_endpoint;
pub mod playlists;
pub mod potoken;
pub mod provider;
pub mod search;
pub mod stream;
pub mod uploads;
pub mod watchshark;
pub mod ytmusic;

use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::paths::Paths;
use cache::Caches;
use stream::{StreamResolver, YtDlpResolver};
use ytmusic::YtMusic;

#[derive(Clone)]
pub struct NetHandle {
    rt: tokio::runtime::Handle,
    client: Arc<YtMusic>,
    resolver: Arc<dyn StreamResolver>,
    caches: Arc<Caches>,
    tokens: Arc<potoken::PoTokens>,
    watchshark: Arc<watchshark::WatchShark>,
}

impl NetHandle {
    pub fn new(rt: tokio::runtime::Handle, paths: &Paths) -> anyhow::Result<Self> {
        let client = YtMusic::new(paths)?;
        let tokens = Arc::new(potoken::PoTokens::new(paths));
        let watchshark = watchshark::WatchShark::new(paths);
        let ytdlp = Arc::new(YtDlpResolver::new(paths, tokens.clone()));
        let native = Arc::new(player_endpoint::PlayerEndpointResolver::new(paths, ytdlp, watchshark.clone()));
        rt.spawn({
            let native = native.clone();
            async move { native.warm().await }
        });
        let resolver: Arc<dyn StreamResolver> = native;
        let caches = Arc::new(Caches::new(paths));
        rt.spawn({
            let (caches, mut auth) = (caches.clone(), client.subscribe_auth());
            async move {
                while auth.changed().await.is_ok() {
                    if matches!(*auth.borrow_and_update(), ytmusic::AuthState::Anonymous) {
                        caches.set_library_playlists(Vec::new());
                    }
                }
            }
        });
        Ok(Self { rt, client, resolver, caches, tokens, watchshark })
    }

    pub fn spawn<F>(&self, fut: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.rt.spawn(fut)
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn StreamResolver>) -> Self {
        self.resolver = resolver;
        self
    }

    pub fn client(&self) -> &Arc<YtMusic> {
        &self.client
    }

    pub fn resolver(&self) -> &Arc<dyn StreamResolver> {
        &self.resolver
    }

    pub fn tokens(&self) -> &Arc<potoken::PoTokens> {
        &self.tokens
    }

    pub fn watchshark(&self) -> &Arc<watchshark::WatchShark> {
        &self.watchshark
    }

    pub fn caches(&self) -> &Arc<Caches> {
        &self.caches
    }
}
