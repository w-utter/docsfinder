use crate::crates_api::Info;
use std::sync::Arc;
use std::sync::mpsc;

use crate::conn_pool::ConnPool;
use crate::connection::CrateError;

fn create_config() -> tokio_rustls::rustls::ClientConfig {
    use tokio_rustls::rustls::{ClientConfig, RootCertStore};

    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

#[derive(Debug)]
pub enum HttpConnectionError {
    Ended,
    Error(hyper::Error),
}

#[derive(Clone)]
pub struct Client {
    discovery_pool: Arc<ConnPool<crate::discovery::DiscoveryManager>>,
    crates_pool: Arc<ConnPool<crate::crates_api::CratesManager>>,
    seen_crates: Arc<lockfree::set::Set<String>>,

    err_tx: mpsc::Sender<Result<Info, CrateError>>,

    user_agent: Arc<str>,
}

pub struct Receiver {
    inner: mpsc::Receiver<Result<Info, CrateError>>,
}

impl Client {
    pub async fn new(user_agent: String) -> reqwest::Result<(Self, Receiver)> {
        let (tx, rx) = mpsc::channel();
        let err_tx = tx.clone();

        let client = {
            let config = Arc::new(create_config());

            let discovery_manager = crate::discovery::DiscoveryManager::new(config.clone());

            use crate::conn_pool::ConnPool;
            use std::time::Duration;
            let discovery_pool = Arc::new(ConnPool::new(discovery_manager, Duration::from_secs(5)));

            let crates_manager = crate::crates_api::CratesManager::new(config.clone());

            let crates_pool = Arc::new(ConnPool::new(crates_manager, Duration::from_secs(5)));

            let seen_crates = Arc::new(lockfree::set::Set::<String>::new());

            Self {
                discovery_pool,
                crates_pool,
                seen_crates,
                err_tx,
                user_agent: user_agent.into(),
            }
        };

        let receiver = Receiver { inner: rx };

        Ok((client, receiver))
    }

    pub async fn spin(self, mut rx: tokio::sync::mpsc::UnboundedReceiver<()>) {
        loop {
            let Some(_) = rx.recv().await else {
                break;
            };

            let discovery_pool = self.discovery_pool.clone();
            let seen_crates = self.seen_crates.clone();
            let crates_pool = self.crates_pool.clone();
            let user_agent = self.user_agent.clone();
            let err_tx = self.err_tx.clone();

            tokio::task::spawn(async move {
                let mut conn = Self::get_connection(&discovery_pool, &err_tx).await;

                loop {
                    let crate_name = match conn.try_get_crate_name().await {
                        Ok(Some(url)) => url,
                        Ok(None) => continue,
                        Err(e) => {
                            if e.io_source() {
                                conn.invalidate();
                                conn = Self::get_connection(&discovery_pool, &err_tx).await;
                            }
                            let _ = err_tx.send(Err(e));

                            continue;
                        }
                    };

                    if seen_crates.contains(&crate_name) {
                        continue;
                    }

                    let _ = seen_crates.insert(crate_name.clone());

                    let crates_pool = crates_pool;

                    tokio::task::spawn(async move {
                        async fn get_body(
                            crates_pool: Arc<
                                crate::conn_pool::ConnPool<crate::crates_api::CratesManager>,
                            >,
                            name: String,
                            user_agent: &str,
                            err_tx: &mpsc::Sender<Result<Info, CrateError>>,
                        ) -> crate::crates_api::Info {
                            let mut conn = Client::get_connection(&crates_pool, &err_tx).await;

                            loop {
                                match conn.try_get_crate(&name, user_agent).await {
                                    Ok(b) => return b,
                                    Err(e) => {
                                        if e.io_source() {
                                            conn.invalidate();
                                            conn =
                                                Client::get_connection(&crates_pool, &err_tx).await;
                                        }

                                        let _ = err_tx.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                        }

                        let body = get_body(crates_pool, crate_name, &user_agent, &err_tx).await;
                        let _ = err_tx.send(Ok(body));
                    });
                    break;
                }
            });
        }
    }

    async fn get_connection<
        M: crate::conn_pool::ConnectionManager<Error = crate::crates_api::InfoErr> + 'static,
    >(
        pool: &Arc<crate::conn_pool::ConnPool<M>>,
        err_tx: &mpsc::Sender<Result<Info, CrateError>>,
    ) -> crate::conn_pool::Connection<M::Conn> {
        loop {
            use crate::conn_pool::ConnectionError;
            match pool.clone().get_connection().await {
                Err(e) => match e {
                    ConnectionError::Creation(e) => {
                        let _ = err_tx.send(Err(CrateError::Crates(e)));
                    }
                    ConnectionError::Timeout => {
                        let _ = err_tx.send(Err(CrateError::Timeout));
                    }
                },
                Ok(c) => {
                    return c;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

impl Receiver {
    pub fn recv(&self) -> Option<Result<Info, CrateError>> {
        use std::sync::mpsc::TryRecvError;
        match self.inner.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                unreachable!()
            }
        }
    }
}
