#[derive(Debug)]
pub enum InfoErr {
    #[allow(unused)]
    Http(reqwest::Error),
    #[allow(unused)]
    Hyper(hyper::Error),
    #[allow(unused)]
    Api(crates_io_api::ApiErrors),
    #[allow(unused)]
    Io(std::io::Error),
    #[allow(unused)]
    Serde(serde_json::Error),
    NotFound,
    Timeout,
}

impl std::error::Error for InfoErr {

}

impl std::fmt::Display for InfoErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Debug>::fmt(self, f)
    }
}

impl From<reqwest::Error> for InfoErr {
    fn from(f: reqwest::Error) -> Self {
        Self::Http(f)
    }
}

impl From<serde_json::Error> for InfoErr {
    fn from(f: serde_json::Error) -> Self {
        Self::Serde(f)
    }
}

impl From<hyper::Error> for InfoErr {
    fn from(f: hyper::Error) -> Self {
        Self::Hyper(f)
    }
}

impl From<std::io::Error> for InfoErr {
    fn from(f: std::io::Error) -> Self {
        Self::Io(f)
    }
}

use chrono::{DateTime, Utc};
#[derive(Debug, Clone)]
pub struct Info {
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub last_update: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub description: Option<String>,
    pub version: String,
    pub stable_version: Option<String>,
    pub downloads: u64,
    pub recent_downloads: Option<u64>,
    pub name: String,
    pub repo_link: Option<String>,
    pub docs_link: Option<String>,
}

impl From<crates_io_api::Crate> for Info {
    fn from(f: crates_io_api::Crate) -> Info {
        let crates_io_api::Crate {
            keywords,
            categories,
            created_at,
            updated_at,
            max_version,
            max_stable_version,
            downloads,
            recent_downloads,
            repository,
            documentation,
            description,
            name,
            ..
        } = f;

        let mut keywords = keywords.unwrap_or_default();
        let mut categories = categories.unwrap_or_default();

        keywords.sort();
        categories.sort();

        Info {
            keywords,
            categories,
            created_at,
            last_update: updated_at,
            version: max_version,
            stable_version: max_stable_version,
            downloads,
            recent_downloads,
            repo_link: repository,
            docs_link: documentation,
            description,
            name,
        }
    }
}

impl From<crates_io_api::CrateResponse> for Info {
    fn from(f: crates_io_api::CrateResponse) -> Info {
        f.crate_data.into()
    }
}

use std::sync::{LazyLock, Arc, atomic::{Ordering, AtomicBool}};
use tokio_rustls::rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;


#[derive(Clone)]
pub struct CratesManager {
    config: Arc<tokio_rustls::rustls::ClientConfig>,
}

impl CratesManager {
    // part of url before the crate name
    const CRATES_URL: &str = "https://crates.io/api/v1/crates/";
    const HTTPS_PORT: u16 = 443;

    pub fn new(config: Arc<ClientConfig>) -> Self {
        Self {
            config
        }
    }
}

pub(crate) static PARSED_CRATES_URL: LazyLock<hyper::Uri> = LazyLock::new(|| hyper::Uri::from_static(CratesManager::CRATES_URL));
pub(crate) static CRATES_HOSTNAME: LazyLock<rustls_pki_types::ServerName<'static>> = LazyLock::new(|| rustls_pki_types::ServerName::try_from(PARSED_CRATES_URL.host().unwrap_or_default().to_string()).unwrap());

impl connection_pool::ConnectionManager for CratesManager {
    type Connection = crate::discovery::Connection;
    type Error = InfoErr;
    type CreateFut = impl Future<Output = Result<Self::Connection, Self::Error>> + Send;
    type ValidFut<'a> = impl Future<Output = bool> + Send + 'a;
    
    fn create_connection(&self) -> Self::CreateFut {
        let config = self.config.clone();
        async {
            let parsed_url = &PARSED_CRATES_URL;
            let host = parsed_url.host().unwrap_or_default();

            let connector = TlsConnector::from(config);
            let stream = TcpStream::connect((host, Self::HTTPS_PORT)).await?;
            let stream = connector.connect(CRATES_HOSTNAME.clone(), stream).await?;

            let io = crate::hyper_tls::Io::new(stream);
            let (inner, conn) = hyper::client::conn::http1::handshake(io).await?;
            let valid = Arc::new(AtomicBool::from(true));

            let is_valid = valid.clone();
            tokio::task::spawn(async move {
                if let Err(err) = conn.await {
                    is_valid.store(false, Ordering::SeqCst);
                    println!("Connection failed: {:?}", err);
                }
            });

            Ok(crate::discovery::Connection {
                inner,
                valid,
            })
        }
    }

    fn is_valid<'a>(&'a self, conn: &'a mut Self::Connection) -> Self::ValidFut<'a> {
        async {
            conn.valid.load(Ordering::Relaxed) && !conn.inner.is_closed()
        }
    }
}
