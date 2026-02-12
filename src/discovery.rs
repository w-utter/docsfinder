#[derive(Clone)]
pub struct DiscoveryManager {
    config: Arc<tokio_rustls::rustls::ClientConfig>,
}

use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;

impl DiscoveryManager {
    pub(crate) const DOCS_RANDOM_URL: &str =
        "https://docs.rs/releases/search?query=&i-am-feeling-lucky=1";
    pub(crate) const HTTPS_PORT: u16 = 443;

    pub fn new(config: Arc<ClientConfig>) -> Self {
        Self { config }
    }
}

use crate::client::HttpConnectionError;
use crate::connection::Connection;

impl crate::conn_pool::ConnectionManager for DiscoveryManager {
    type Conn = Connection;
    type Error = crate::crates_api::InfoErr;

    async fn create_connection(&self) -> Result<Self::Conn, Self::Error> {
        let config = self.config.clone();
        let parsed_url = &crate::connection::PARSED_URL;
        let host = parsed_url.host().unwrap_or_default();

        let connector = TlsConnector::from(config);
        let stream = TcpStream::connect((host, Self::HTTPS_PORT)).await?;
        let stream = connector
            .connect(crate::connection::HOSTNAME.clone(), stream)
            .await?;

        let io = crate::hyper_tls::Io::new(stream);
        let (inner, conn) = hyper::client::conn::http1::handshake(io).await?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::task::spawn(async move {
            if let Err(err) = conn.await {
                let _ = tx.send(HttpConnectionError::Error(err));
            } else {
                let _ = tx.send(HttpConnectionError::Ended);
            }
        });

        Ok(Connection { inner, notify: rx })
    }
}
