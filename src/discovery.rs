

#[derive(Clone)]
pub struct DiscoveryManager {
    config: Arc<tokio_rustls::rustls::ClientConfig>,
}

use std::sync::{LazyLock, Arc};
use tokio_rustls::rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

impl DiscoveryManager {
    const DOCS_RANDOM_URL: &str = "https://docs.rs/releases/search?query=&i-am-feeling-lucky=1";
    const HTTPS_PORT: u16 = 443;

    pub fn new(config: Arc<ClientConfig>) -> Self {
        Self {
            config
        }
    }
}

use std::sync::atomic::{AtomicBool, Ordering};

pub struct Connection {
    pub valid: Arc<AtomicBool>,
    pub inner: hyper::client::conn::http1::SendRequest<http_body_util::Empty<hyper::body::Bytes>>,
}

impl Connection {
    pub async fn get_crate_name(&mut self) -> Result<Option<String>, crate::crates_api::InfoErr> {
        use http_body_util::Empty;
        use hyper::body::Bytes;

        let authority = PARSED_URL.authority().map(|a| a.as_str()).unwrap_or_default();

        let req = hyper::Request::builder()
            .uri(&*PARSED_URL)
            .header(hyper::header::HOST, authority)
            .body(Empty::<Bytes>::new()).unwrap();

        let res = self.inner.send_request(req).await?;

        Ok(res.headers().get("location").map(|l| l.to_str()).transpose().unwrap().map(|s| s.split('/').filter(|s| !s.is_empty()).next().unwrap().to_string()))
    }

    pub async fn get_crate(&mut self, crate_name: &str, user_agent: &str) -> Result<crate::crates_api::Info, crate::crates_api::InfoErr> {
        use http_body_util::Empty;
        use hyper::body::Bytes;

        let uri = {
            let mut parts = crate::crates_api::PARSED_CRATES_URL.clone().into_parts();
            let path = format!("/api/v1/crates/{crate_name}");
            parts.path_and_query = Some(hyper::http::uri::PathAndQuery::from_maybe_shared(path).unwrap());
            hyper::http::uri::Uri::from_parts(parts).unwrap()
        };

        let authority = crate::crates_api::PARSED_CRATES_URL.authority().map(|a| a.as_str()).unwrap_or_default();

        let req = hyper::Request::builder()
            .uri(uri)
            .header(hyper::header::HOST, authority)
            .header(hyper::header::USER_AGENT, user_agent)
            .body(Empty::<Bytes>::new()).unwrap();

        let res = self.inner.send_request(req).await?;
        use http_body_util::BodyExt;
        let body = res.collect().await?.aggregate();

        use hyper::body::Buf;

        use std::io::Read;
        let mut reader = Vec::new();
        body.reader().read_to_end(&mut reader)?;

        if let Ok(err) = serde_json::from_reader::<_, crates_io_api::ApiErrors>(reader.as_slice()) {
            return Err(crate::InfoErr::Api(err));
        }

        let parsed = serde_json::from_reader::<_, crates_io_api::CrateResponse>(reader.as_slice())?;
        let info: crate::crates_api::Info = parsed.into();

        Ok(info)
    }
}

static PARSED_URL: LazyLock<hyper::Uri> = LazyLock::new(|| hyper::Uri::from_static(DiscoveryManager::DOCS_RANDOM_URL));
static HOSTNAME: LazyLock<rustls_pki_types::ServerName<'static>> = LazyLock::new(|| rustls_pki_types::ServerName::try_from(PARSED_URL.host().unwrap_or_default().to_string()).unwrap());

impl connection_pool::ConnectionManager for DiscoveryManager {
    type Connection = Connection;
    type Error = crate::crates_api::InfoErr;
    type CreateFut = impl Future<Output = Result<Connection, Self::Error>> + Send;
    type ValidFut<'a> = impl Future<Output = bool> + Send + 'a;
    
    fn create_connection(&self) -> Self::CreateFut {
        let config = self.config.clone();
        async {
            let parsed_url = &PARSED_URL;
            let host = parsed_url.host().unwrap_or_default();

            let connector = TlsConnector::from(config);
            let stream = TcpStream::connect((host, Self::HTTPS_PORT)).await?;
            let stream = connector.connect(HOSTNAME.clone(), stream).await?;

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

            Ok(Connection {
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
