use std::{
    fs,
    io::ErrorKind,
    path::Path,
    sync::{
        Arc,
        Mutex,
    },
    time::Duration,
    num::NonZeroU32,
    collections::HashMap,
};
use bread_common::projectconfig::{
    self,
    VersionedProjectConfig,
    FILENAME,
};
use anyhow::{
    anyhow,
    Result,
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
};
use slog::Logger;
use crate::{
    aes,
    trace,
};

pub const DEFAULT_WEIGHT: u32 = 100;

#[derive(Clone)]
pub struct Context {
    hc: Client,
    pub limiters: Arc<
        Mutex<HashMap<String, Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware>>>>,
    >,
    pub config: Arc<Mutex<projectconfig::latest::Config>>,
}

impl Context {
    pub fn new(config: Option<projectconfig::latest::Config>) -> Self {
        Context {
            hc: reqwest::Client::builder().user_agent("https://github.com/andrewbaxter/bread-scan").build().unwrap(),
            config: Arc::new(Mutex::new(config.unwrap_or_else(|| projectconfig::latest::Config {
                disabled: false,
                weights: projectconfig::v1::Weights {
                    accounts: Default::default(),
                    projects: Default::default(),
                },
            }))),
            limiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get(&self, url: &str) -> Result<RequestBuilder> {
        let url = url::Url::parse(url)?;
        let limiter =
            self
                .limiters
                .lock()
                .unwrap()
                .entry(url.host_str().unwrap_or("").to_string())
                .or_insert_with(
                    || Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(5u32).unwrap()))),
                )
                .clone();
        limiter.until_ready_with_jitter(Jitter::up_to(Duration::from_millis(500))).await;
        Ok(self.hc.get(url))
    }

    pub async fn add_url_canonicalize(&self, log: &Logger, url: &str) {
        async fn canonicalize(log: &Logger, ctx: &Context, raw_url: &str) -> String {
            match aes!({
                let text =
                    ctx
                        .get(raw_url)
                        .await?
                        .header(
                            header::ACCEPT,
                            HeaderValue::from_static(
                                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                            ),
                        )
                        .send()
                        .await?
                        .text()
                        .await?;
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
                        "Error extracting canonical url from page",
                        err = e.to_string(),
                        url = raw_url.to_string()
                    );
                },
            }
            let url = match url::Url::parse(raw_url) {
                Err(e) => {
                    trace!(
                        log,
                        "Error parsing url during canonicalization",
                        err = e.to_string(),
                        url = raw_url.to_string()
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

        let url = canonicalize(log, self, url).await;
        self.config.lock().unwrap().weights.projects.insert(url, DEFAULT_WEIGHT);
    }
}

pub async fn process_bread(ctx: &Context, path: &Path) {
    match aes!({
        let config: VersionedProjectConfig = serde_yaml::from_slice(&match maybe_read(&path.join(FILENAME)) {
            Err(e) => {
                return Err(e.into());
            },
            Ok(None) => {
                return Ok(());
            },
            Ok(Some(b)) => b,
        })?;
        match config {
            VersionedProjectConfig::V1(config) => {
                let mut write = ctx.config.lock().unwrap();
                write.weights.accounts.extend(config.weights.accounts);
                write.weights.projects.extend(config.weights.projects);
            },
        };
        Ok(())
    }).await {
        Ok(_) => { },
        Err(e) => {
            eprintln!("Error processing submodule at [[{}]]: {}", path.to_string_lossy(), e);
        },
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

pub fn norm_repo(repo: &str) -> Result<Option<String>> {
    let url = url::Url::parse(repo)?;
    let host = url.host_str().ok_or(anyhow!("Missing host portion of url"))?.to_string();
    let mut path: Vec<String> = url.path().split('/').map(|s| s.to_string()).collect();
    let mut matched = false;
    if host.starts_with("github.com") {
        path.truncate(3);
        matched = true;
    }
    if host.starts_with("gitlab.com") {
        path.truncate(3);
        matched = true;
    }
    if host.starts_with("sr.ht") {
        path.truncate(3);
        matched = true;
    }
    if !matched {
        return Ok(None);
    }
    Ok(Some(format!("https://{}{}", host, path.join("/"))))
}
