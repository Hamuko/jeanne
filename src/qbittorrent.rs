use crate::config;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use url::Url;

const GLOBAL_LIMIT: &str = "-2";

fn value_or_global_limit<T: ToString>(value: Option<T>) -> Cow<'static, str> {
    match value {
        Some(value) => value.to_string().into(),
        None => GLOBAL_LIMIT.into(),
    }
}

const URL_FAILURE: &str = "Could not build URL";

pub type Ratio = f64;
pub type MaxSeedingTime = i32;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagList(Vec<String>);

impl From<String> for TagList {
    fn from(item: String) -> Self {
        Self(item.split_terminator(',').map(|x| x.to_string()).collect())
    }
}

impl fmt::Display for TagList {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{}]", self.0.join(", "))
    }
}

type TorrentMap = HashMap<String, Torrent>;

#[derive(Debug)]
pub enum AuthenticationError {
    Banned,
    Credentials,
    MissingCredentials,
    Request(reqwest::Error),
}

impl fmt::Display for AuthenticationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Banned => write!(f, "This IP is banned for too many login attempts"),
            Self::Credentials => write!(f, "Could not log in to server"),
            Self::MissingCredentials => write!(f, "Username and password are not set"),
            Self::Request(reqwest_error) => write!(f, "HTTP client error: {}", reqwest_error),
        }
    }
}

#[derive(Debug)]
pub enum ClientError {
    Authentication,
    BadRequest,
    InvalidUrl,
    Reqwest(reqwest::Error),
}

pub struct Client {
    base_url: Url,
    client: reqwest::Client,
    password: Option<String>,
    rid: usize,
    pub torrents: TorrentMap,
    pub username: Option<String>,
}

impl Client {
    pub async fn login(&self) -> Result<(), AuthenticationError> {
        let username = self
            .username
            .as_ref()
            .ok_or(AuthenticationError::MissingCredentials)?;
        let password = self
            .password
            .as_ref()
            .ok_or(AuthenticationError::MissingCredentials)?;
        log::debug!("Logging in as {}", username);
        let url = self.base_url.join("api/v2/auth/login").expect(URL_FAILURE);
        let response = self
            .client
            .clone()
            .post(url)
            .form(&[("username", username), ("password", password)])
            .send()
            .await
            .map_err(AuthenticationError::Request)?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(AuthenticationError::Banned);
        }
        if !response.headers().contains_key(reqwest::header::SET_COOKIE) {
            return Err(AuthenticationError::Credentials);
        }
        log::info!("Logged in as {}", username);
        Ok(())
    }

    pub fn new(config: config::ServerConfig) -> Result<Self, ClientError> {
        let base_url = Url::parse(&config.address).map_err(|_| ClientError::InvalidUrl)?;
        if (base_url.scheme() != "http" && base_url.scheme() != "https")
            || base_url.cannot_be_a_base()
        {
            return Err(ClientError::InvalidUrl);
        }

        let client = reqwest::Client::builder()
            .cookie_store(true)
            .referer(true)
            .build()
            .map_err(ClientError::Reqwest)?;
        Ok(Self {
            base_url,
            client,
            password: config.password,
            rid: 0,
            torrents: HashMap::new(),
            username: config.username,
        })
    }

    pub async fn update(&mut self) -> Result<(), ClientError> {
        log::trace!("Syncing data");
        let url = self
            .base_url
            .join("api/v2/sync/maindata")
            .expect(URL_FAILURE);
        let response = self
            .client
            .clone()
            .get(url)
            .query(&[("rid", self.rid)])
            .send()
            .await
            .map_err(ClientError::Reqwest)?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(ClientError::Authentication);
        }
        let main_data = response
            .json::<MainData>()
            .await
            .map_err(ClientError::Reqwest)?;
        if main_data.full_update.is_some() {
            log::debug!("Received a full update from server");
            self.torrents = main_data
                .torrents
                .into_iter()
                .filter_map(|(k, v)| match Torrent::from_data(v) {
                    Ok(torrent) => Some((k, torrent)),
                    Err(error) => {
                        log::warn!("Unable to deserialize torrent: missing {}", error);
                        None
                    }
                })
                .collect();
        } else {
            if let Some(torrents_removed) = main_data.torrents_removed {
                for hash in torrents_removed {
                    if self.torrents.remove(&hash).is_some() {
                        log::trace!("Removed torrent {}", hash);
                    };
                }
            }
            for (key, data) in main_data.torrents {
                if let Some(torrent) = self.torrents.get_mut(&key) {
                    log::trace!("Updating {}", key);
                    torrent.update(data);
                } else {
                    log::trace!("Inserting {}", key);
                    match Torrent::from_data(data) {
                        Ok(torrent) => {
                            self.torrents.insert(key, torrent);
                        }
                        Err(field) => {
                            log::warn!("Could not load torrent {}: no {} field", key, field);
                        }
                    };
                }
            }
        }

        self.rid = main_data.rid;
        log::trace!("Data synced");
        Ok(())
    }

    pub async fn apply_rule_limits(
        &self,
        hash: &str,
        limits: &config::RuleLimits,
    ) -> Result<(), ClientError> {
        self.set_share_limits(hash, limits.ratio, limits.minutes)
            .await
    }

    pub async fn apply_global_limits(&self, hash: &str) -> Result<(), ClientError> {
        self.set_share_limits(hash, None, None).await
    }

    async fn set_share_limits(
        &self,
        hash: &str,
        ratio: Option<Ratio>,
        minutes: Option<MaxSeedingTime>,
    ) -> Result<(), ClientError> {
        let ratio = value_or_global_limit(ratio);
        let minutes = value_or_global_limit(minutes);
        let data = HashMap::from([
            ("hashes", hash),
            ("inactiveSeedingTimeLimit", GLOBAL_LIMIT),
            ("ratioLimit", &ratio),
            ("seedingTimeLimit", &minutes),
        ]);
        let url = self
            .base_url
            .join("api/v2/torrents/setShareLimits")
            .expect(URL_FAILURE);
        let response = self
            .client
            .clone()
            .post(url)
            .form(&data)
            .send()
            .await
            .map_err(ClientError::Reqwest)?;
        if response.status() == reqwest::StatusCode::OK {
            return Ok(());
        }
        Err(ClientError::BadRequest)
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MainData {
    full_update: Option<bool>,
    rid: usize,
    torrents: HashMap<String, PartialTorrent>,
    torrents_removed: Option<Vec<String>>,
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct Torrent {
    pub category: String,
    pub max_ratio: Ratio,
    pub max_seeding_time: MaxSeedingTime,
    pub name: String,
    pub seeding_time: usize,
    pub tags: TagList,
}

#[derive(Debug)]
enum TorrentField {
    Category,
    MaxRatio,
    MaxSeedingTime,
    Name,
    SeedingTime,
    Tags,
}

impl fmt::Display for TorrentField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let name = match self {
            Self::Category => "category",
            Self::MaxRatio => "max_ratio",
            Self::MaxSeedingTime => "max_seeding_time",
            Self::Name => "name",
            Self::SeedingTime => "seeding_time",
            Self::Tags => "tags",
        };
        write!(f, "{}", name)
    }
}

impl Torrent {
    pub fn is_limited(&self) -> bool {
        self.max_seeding_time >= 0 || self.max_ratio >= 0.0
    }

    fn from_data(torrent_data: PartialTorrent) -> Result<Self, TorrentField> {
        let category = torrent_data.category.ok_or(TorrentField::Category)?;
        let max_ratio = torrent_data.max_ratio.ok_or(TorrentField::MaxRatio)?;
        let max_seeding_time = torrent_data
            .max_seeding_time
            .ok_or(TorrentField::MaxSeedingTime)?;
        let name = torrent_data.name.ok_or(TorrentField::Name)?;
        let seeding_time = torrent_data.seeding_time.ok_or(TorrentField::SeedingTime)?;
        let tags = TagList::from(torrent_data.tags.ok_or(TorrentField::Tags)?);
        Ok(Self {
            category,
            max_ratio,
            max_seeding_time,
            name,
            seeding_time,
            tags,
        })
    }

    fn update(&mut self, torrent_data: PartialTorrent) {
        if let Some(category) = torrent_data.category {
            self.category = category
        }
        if let Some(max_ratio) = torrent_data.max_ratio {
            self.max_ratio = max_ratio
        }
        if let Some(max_seeding_time) = torrent_data.max_seeding_time {
            self.max_seeding_time = max_seeding_time
        }
        if let Some(name) = torrent_data.name {
            self.name = name
        }
        if let Some(seeding_time) = torrent_data.seeding_time {
            self.seeding_time = seeding_time
        }
        if let Some(tags) = torrent_data.tags {
            self.tags = TagList::from(tags)
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PartialTorrent {
    category: Option<String>,
    max_ratio: Option<Ratio>,
    max_seeding_time: Option<MaxSeedingTime>,
    name: Option<String>,
    seeding_time: Option<usize>,
    tags: Option<String>,
}
