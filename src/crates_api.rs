#[derive(Debug)]
pub enum InfoErr {
    Http(reqwest::Error),
    Api(crates_io_api::ApiErrors),
    Serde(serde_json::Error),
    NotFound,
    Timeout,
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

pub(crate) async fn get_crate_info(
    crates_url: String,
    crates_client: &reqwest::Client,
) -> Result<Info, InfoErr> {
    let res = crates_client.get(crates_url).send().await?;
    let krate = res.text().await?;

    if let Ok(err) = serde_json::from_str::<crates_io_api::ApiErrors>(&krate) {
        return Err(InfoErr::Api(err));
    };

    let parsed: crates_io_api::CrateResponse = serde_json::from_str(&krate)?;
    let info: Info = parsed.into();

    Ok(info)
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
