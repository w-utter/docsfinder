use crate::crates_api::{Info, InfoErr};
use std::sync::mpsc;
use std::sync::Arc;

fn create_config() -> tokio_rustls::rustls::ClientConfig {
    use tokio_rustls::rustls::{ClientConfig, RootCertStore};

    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    ClientConfig::builder().with_root_certificates(root_store).with_no_client_auth()
}

#[derive(Clone)]
pub struct Client {
    discovery_pool: Arc<connection_pool::ConnectionPool<crate::discovery::DiscoveryManager>>,
    crates_pool: Arc<connection_pool::ConnectionPool<crate::crates_api::CratesManager>>,
    seen_crates: Arc<lockfree::set::Set<String>>,

    err_tx: mpsc::Sender<Result<Info, InfoErr>>,

    user_agent: Arc<str>,
}

pub struct Receiver {
    inner: mpsc::Receiver<Result<Info, InfoErr>>,
}

impl Client {
    pub async fn new(user_agent: String) -> reqwest::Result<(Self, Receiver)> {
        let (tx, rx) = mpsc::channel();
        let err_tx = tx.clone();

        let client = {
            let config = Arc::new(create_config());


            let discovery_manager = crate::discovery::DiscoveryManager::new(config.clone());

            let discovery_pool = connection_pool::ConnectionPool::new(
                Some(tokio::sync::Semaphore::MAX_PERMITS),
                None,
                None,
                None,
                discovery_manager,
            );

            let crates_manager = crate::crates_api::CratesManager::new(config.clone());
            let crates_pool = connection_pool::ConnectionPool::new(
                Some(tokio::sync::Semaphore::MAX_PERMITS),
                None,
                None,
                None,
                crates_manager,
            );

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
                let mut conn = Self::get_connection(&discovery_pool).await;

                loop {
                    let crate_name = match conn.get_crate_name().await {
                        Ok(Some(url)) => url,
                        Ok(None) => continue,
                        Err(_) => {
                            conn = Self::get_connection(&discovery_pool).await;
                            continue;
                        }
                    };

                    if seen_crates.contains(&crate_name) {
                        continue;
                    }

                    let _ = seen_crates.insert(crate_name.clone());

                    let crates_pool = crates_pool;

                    tokio::task::spawn(async move {
                        async fn get_body(crates_pool: Arc<connection_pool::ConnectionPool<crate::crates_api::CratesManager>>, name: String, user_agent: &str) -> crate::crates_api::Info {
                            let mut conn = Client::get_connection(&crates_pool).await;

                            loop {
                                match conn.get_crate(&name, user_agent).await {
                                    Ok(b) => return b,
                                    Err(e) => {
                                        println!("err: {e:?}");
                                        conn = Client::get_connection(&crates_pool).await;
                                        continue;
                                    }
                                }
                            }
                        }

                        let body = get_body(crates_pool, crate_name, &user_agent).await;
                        let _ = err_tx.send(Ok(body));
                    });
                    break;
                }
            });
        }
    }

    async fn get_connection<M: connection_pool::ConnectionManager + 'static>(pool: &Arc<connection_pool::ConnectionPool<M>>) -> connection_pool::ManagedConnection<M> {
        loop {
            match pool.clone().get_connection().await {
                Err(_) => (),
                Ok(c) => {
                    return c;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

impl Receiver {
    pub fn recv(&self) -> Option<Result<Info, InfoErr>> {
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

