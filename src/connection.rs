use crate::client::HttpConnectionError;
use crate::discovery::DiscoveryManager;
use std::sync::LazyLock;

pub(crate) static PARSED_URL: LazyLock<hyper::Uri> =
    LazyLock::new(|| hyper::Uri::from_static(DiscoveryManager::DOCS_RANDOM_URL));
pub(crate) static HOSTNAME: LazyLock<rustls_pki_types::ServerName<'static>> = LazyLock::new(|| {
    rustls_pki_types::ServerName::try_from(PARSED_URL.host().unwrap_or_default().to_string())
        .unwrap()
});

pub struct Connection {
    pub notify: tokio::sync::oneshot::Receiver<HttpConnectionError>,
    pub inner: hyper::client::conn::http1::SendRequest<http_body_util::Empty<hyper::body::Bytes>>,
}

#[derive(Debug)]
pub enum CrateError {
    Http(HttpConnectionError),
    Crates(crate::crates_api::InfoErr),
    Timeout,
}

impl CrateError {
    pub fn io_source(&self) -> bool {
        use crate::crates_api::InfoErr;
        matches!(
            self,
            Self::Http(_) | Self::Crates(InfoErr::Hyper(_) | InfoErr::Io(_))
        )
    }
}

impl Connection {
    pub async fn try_get_crate_name(&mut self) -> Result<Option<String>, CrateError> {
        tokio::select! {
            closed = &mut self.notify => Err(CrateError::Http(closed.unwrap_or(HttpConnectionError::Ended))),
            krate = Self::get_crate_name(&mut self.inner) => {
                krate.map_err(|e| CrateError::Crates(e))
            }
        }
    }

    async fn get_crate_name(
        this: &mut hyper::client::conn::http1::SendRequest<
            http_body_util::Empty<hyper::body::Bytes>,
        >,
    ) -> Result<Option<String>, crate::crates_api::InfoErr> {
        use http_body_util::Empty;
        use hyper::body::Bytes;

        let authority = PARSED_URL
            .authority()
            .map(|a| a.as_str())
            .unwrap_or_default();

        let req = hyper::Request::builder()
            .uri(&*PARSED_URL)
            .header(hyper::header::HOST, authority)
            .body(Empty::<Bytes>::new())
            .unwrap();

        let res = this.send_request(req).await?;

        Ok(res
            .headers()
            .get("location")
            .map(|l| l.to_str())
            .transpose()
            .unwrap()
            .map(|s| {
                s.split('/')
                    .filter(|s| !s.is_empty())
                    .next()
                    .unwrap()
                    .to_string()
            }))
    }

    pub async fn try_get_crate(
        &mut self,
        crate_name: &str,
        user_agent: &str,
    ) -> Result<crate::crates_api::Info, CrateError> {
        tokio::select! {
            closed = &mut self.notify => Err(CrateError::Http(closed.unwrap_or(HttpConnectionError::Ended))),
            krate = Self::get_crate(&mut self.inner, crate_name, user_agent) => {
                krate.map_err(|e| CrateError::Crates(e))
            }
        }
    }

    async fn get_crate(
        this: &mut hyper::client::conn::http1::SendRequest<
            http_body_util::Empty<hyper::body::Bytes>,
        >,
        crate_name: &str,
        user_agent: &str,
    ) -> Result<crate::crates_api::Info, crate::crates_api::InfoErr> {
        use http_body_util::Empty;
        use hyper::body::Bytes;

        let uri = {
            let mut parts = crate::crates_api::PARSED_CRATES_URL.clone().into_parts();
            let path = format!("/api/v1/crates/{crate_name}");
            parts.path_and_query =
                Some(hyper::http::uri::PathAndQuery::from_maybe_shared(path).unwrap());
            hyper::http::uri::Uri::from_parts(parts).unwrap()
        };

        let authority = crate::crates_api::PARSED_CRATES_URL
            .authority()
            .map(|a| a.as_str())
            .unwrap_or_default();

        let req = hyper::Request::builder()
            .uri(uri)
            .header(hyper::header::HOST, authority)
            .header(hyper::header::USER_AGENT, user_agent)
            .body(Empty::<Bytes>::new())
            .unwrap();

        let res = this.send_request(req).await?;
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
