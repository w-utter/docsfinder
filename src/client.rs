use crate::crates_api::{self, Info, InfoErr};
use std::sync::mpsc;

#[derive(Clone)]
pub struct Client {
    inner: reqwest::Client,
    err_tx: mpsc::Sender<Result<Info, InfoErr>>,
}

pub struct Receiver {
    inner: mpsc::Receiver<Result<Info, InfoErr>>,
}

impl Client {
    pub fn new(user_agent: &str) -> reqwest::Result<(Self, Receiver)> {
        let crates_client = reqwest::Client::builder().user_agent(user_agent).build()?;

        let (tx, rx) = mpsc::channel();
        let err_tx = tx.clone();

        let redirect_scheme = reqwest::redirect::Policy::custom(move |attempt| {
            let crates_client = crates_client.clone();
            let url = attempt.url();

            let Some((crate_name, _)) = crates_info_from_url_path(url.path()) else {
                let _ = tx.send(Err(InfoErr::NotFound));
                return attempt.stop();
            };

            let crates_url = format!("https://crates.io/api/v1/crates/{crate_name}");

            let tx_2 = tx.clone();

            tokio::task::spawn(async move {
                let info = crates_api::get_crate_info(crates_url, &crates_client).await;
                let _ = tx_2.send(info);
            });

            attempt.stop()
        });

        let redirect_client = reqwest::Client::builder()
            .redirect(redirect_scheme)
            .build()?;

        let client = Self {
            inner: redirect_client,
            err_tx,
        };

        let receiver = Receiver { inner: rx };

        Ok((client, receiver))
    }

    pub async fn spin(self, mut rx: tokio::sync::mpsc::UnboundedReceiver<()>) {
        loop {
            let Some(_) = rx.recv().await else {
                break;
            };

            let this = self.clone();

            tokio::task::spawn(async move {
                this.new_entry().await;
            });
        }
    }

    async fn new_entry(&self) {
        const DOCS_RANDOM_URL: &str = "https://docs.rs/releases/search?query=&i-am-feeling-lucky=1";

        use std::time::Duration;
        const TIMEOUT_DURATION: Duration = Duration::from_secs(30);

        match tokio::time::timeout(TIMEOUT_DURATION, self.inner.get(DOCS_RANDOM_URL).send()).await {
            Err(_) => {
                let _ = self.err_tx.send(Err(InfoErr::Timeout));
            }
            Ok(Err(e)) => {
                let _ = self.err_tx.send(Err(InfoErr::Http(e)));
            }
            Ok(Ok(_)) => (),
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

fn crates_info_from_url_path(path: &str) -> Option<(&str, &str)> {
    let mut iter = path.split("/").filter(|s| !s.is_empty());
    let name = iter.next()?;
    let version = iter.next()?;
    Some((name, version))
}
