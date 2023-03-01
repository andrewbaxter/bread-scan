use std::{
    fs,
    io::ErrorKind,
    path::{
        Path,
        PathBuf,
    },
    sync::{
        Arc,
        Mutex,
    },
    time::Duration,
    num::NonZeroU32,
    collections::HashMap,
};
use bread_common::{
    AccountId,
};
use anyhow::{
    anyhow,
    Result,
    Context as _,
};
use governor::{
    RateLimiter,
    state::{
        NotKeyed,
        InMemoryState,
    },
    clock::QuantaClock,
    middleware::NoOpMiddleware,
    Jitter,
    Quota,
};
use reqwest::{
    Client,
    RequestBuilder,
    header::{
        self,
        HeaderValue,
    },
    Url,
};
use serde::{
    Deserialize,
    Serialize,
};
use slog::{
    Logger,
    trace,
    warn,
};
use crate::{
    aes,
};

pub const DEFAULT_WEIGHT: u32 = 100;
pub const USER_AGENT: &'static str = "https://github.com/andrewbaxter/bread-scan";

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct WorkingAccount {
    pub memo: String,
    pub weight: Option<u32>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct WorkingWeights {
    pub accounts: HashMap<AccountId, WorkingAccount>,
    pub projects: HashMap<String, Option<u32>>,
}

#[derive(Clone)]
pub struct Supercontext {
    cache_path: PathBuf,
    hc: Client,
    pub limiters: Arc<
        Mutex<HashMap<String, Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware>>>>,
    >,
}

impl Supercontext {
    pub fn new(cache_path: PathBuf) -> Self {
        Supercontext {
            cache_path: cache_path,
            hc: reqwest::Client::builder().user_agent(USER_AGENT).build().unwrap(),
            limiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub trait LogErr<T> {
    fn log(self, log: &Logger) -> Option<T>;
}

impl<T> LogErr<T> for std::result::Result<T, anyhow::Error> {
    fn log(self, log: &Logger) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                warn!(log, "{:?}", e);
                None
            },
        }
    }
}

#[derive(Clone)]
pub struct Context {
    pub supercontext: Supercontext,
    pub config: Arc<Mutex<WorkingWeights>>,
}

impl Context {
    pub fn new(supercontext: Supercontext) -> Self {
        Context {
            supercontext: supercontext,
            config: Arc::new(Mutex::new(WorkingWeights::default())),
        }
    }

    pub async fn get(&self, url: &str) -> Result<RequestBuilder> {
        let url = url::Url::parse(url)?;
        let limiter =
            self
                .supercontext
                .limiters
                .lock()
                .unwrap()
                .entry(url.host_str().unwrap_or("").to_string())
                .or_insert_with(
                    || Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(5u32).unwrap()))),
                )
                .clone();
        limiter.until_ready_with_jitter(Jitter::up_to(Duration::from_millis(500))).await;
        Ok(self.supercontext.hc.get(url))
    }

    pub async fn get_html(&self, url: &str) -> Result<String> {
        let text =
            self
                .get(url)
                .await?
                .header(
                    header::ACCEPT,
                    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
                )
                .send()
                .await?
                .text()
                .await?;
        Ok(text)
    }

    pub async fn cache_get(&self, log: &Logger, key: &str) -> Option<String> {
        match cacache::read(&self.supercontext.cache_path, key).await {
            Err(cacache::Error::EntryNotFound(_, _)) => {
                return None;
            },
            v => v,
        }
            .context("Error reading cache")
            .log(log)
            .and_then(|r| String::from_utf8(r).context("Corrupt cache, value not utf8").log(log))
    }

    pub async fn cache_put(&self, log: &Logger, key: &str, value: &str) {
        cacache::write(&self.supercontext.cache_path, key, value).await.context("Error caching url").log(log);
    }

    pub async fn add_url_canonicalize(&self, log: &Logger, raw_url: &str) {
        async fn canonicalize(log: &Logger, ctx: &Context, raw_url: &str) -> String {
            match aes!({
                let text = ctx.get_html(raw_url).await?;
                let page = scraper::Html::parse_document(&text);
                let link = match page.select(&scraper::Selector::parse("link[rel=\"canonical\"]").unwrap()).next() {
                    None => return Ok(None),
                    Some(x) => x,
                };
                Ok(link.value().attr("href").map(|u| u.to_string()))
            }).await {
                Ok(Some(u)) => {
                    return u
                },
                Ok(None) => { },
                Err(e) => {
                    trace!(
                        log,
                        "Error extracting canonical url from page";
                        "err" => e.to_string(),
                        "url" => raw_url.to_string()
                    );
                },
            }
            let url = match url::Url::parse(raw_url) {
                Err(e) => {
                    trace!(
                        log,
                        "Error parsing url during canonicalization";
                        "err" => e.to_string(),
                        "url" => raw_url.to_string()
                    );
                    return raw_url.to_string()
                },
                Ok(u) => u,
            };
            format!(
                "{}://{}{}{}",
                url.scheme(),
                url.host_str().unwrap_or(""),
                url.port().map(|p| format!(":{}", p)).unwrap_or_else(|| "".to_string()),
                url.path()
            )
        }

        let cache_key = format!("canonical-{}", raw_url);
        let url = match self.cache_get(log, &cache_key).await {
            Some(v) => v,
            None => {
                let url = canonicalize(log, self, raw_url).await;
                self.cache_put(log, &cache_key, &url).await;
                url
            },
        };
        self.config.lock().unwrap().projects.insert(url, None);
    }

    pub async fn maybe_add_url(&self, log: &Logger, url: &str) -> bool {
        match aes!({
            let url = Url::parse(url)?;
            let host = url.host_str().ok_or_else(|| anyhow!("URL missing host"))?;
            if host.ends_with(".github.io") {
                let org = host.split(".").next().unwrap();
                self.add_url_canonicalize(&log, &format!("https://github.com/{}{}", org, url.path())).await;
                return Ok(true);
            }
            if ["github.com", "gitlab.com", "sr.ht"].into_iter().any(|d| host.ends_with(d)) ||
                host.split(".").any(|s| s == "gitlab") {
                let mut path: Vec<String> = url.path().split('/').map(|s| s.to_string()).collect();
                let mut matched = false;
                if host == "github.com" {
                    path.truncate(3);
                    matched = true;
                }
                if host == "gitlab.com" {
                    path.truncate(3);
                    matched = true;
                }
                if host == "sr.ht" {
                    path.truncate(3);
                    matched = true;
                }
                if matched {
                    self.add_url_canonicalize(log, &format!("https://{}{}", host, path.join("/"))).await;
                }
                return Ok(true);
            }
            Ok(false)
        }).await {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    log,
                    "Error parsing url";
                    "url" => url.to_string(),
                    "err" => #? e,
                );
                false
            },
        }
    }
}

pub fn maybe_read(p: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(&p) {
        Err(e) => {
            if e.kind() == ErrorKind::NotFound || e.raw_os_error().unwrap_or_default() == 20 {
                // 20 is NotADirectory, enum only on unstable (nop)
                return Ok(None);
            } else {
                return Err(e.into());
            }
        },
        Ok(r) => return Ok(Some(r)),
    }
}
