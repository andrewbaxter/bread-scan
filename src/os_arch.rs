use anyhow::{
    Result,
};
use slog::{
    Logger,
    warn,
    o,
    debug,
};
use structre::structre;
use tokio::{
    process::Command,
    spawn,
};
use crate::{
    common::{
        WorkingWeights,
        Supercontext,
        Context,
    },
    aes,
};

#[structre(r#"(?P<key>[^:\s]+)\s+: (?P<value>.+)"#)]
struct PacmanListKV {
    key: String,
    value: String,
}

pub async fn process(log: Logger, supercontext: Supercontext) -> Result<WorkingWeights> {
    let ctx = Context::new(supercontext.clone());
    let res = Command::new("pacman").args(["--query", "--explicit", "--info"]).output().await?;
    let mut sub_pool = vec![];
    let kv_parser = PacmanListKVFromRegex::new();
    for line in String::from_utf8_lossy(&res.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let kv = match kv_parser.parse(line) {
            Ok(kv) => kv,
            Err(e) => {
                warn!(
                    log,
                    "Error parsing pacman output line";
                    "line" => line.to_string(),
                    "err" => #? e,
                );
                continue;
            },
        };
        if kv.key == "URL" {
            let url = kv.value;
            if ctx.maybe_add_url(&log, &url).await {
                // nop
            } else {
                let log = log.new(o!("url" => url.to_string()));
                let ctx = ctx.clone();
                sub_pool.push(spawn(async move {
                    let cache_key = format!("arch-html-{}", url);
                    let hrefs: Vec<String> = match ctx.cache_get(&log, &cache_key).await {
                        Some(v) => v,
                        None => {
                            let mut hrefs = vec![];
                            aes!({
                                let text = match ctx.http_get_html(&url).await {
                                    Ok(t) => t,
                                    Err(e) => {
                                        debug!(
                                            log,
                                            "Error fetching project page in search for git URL";
                                            "err" => #? e
                                        );
                                        return Ok(());
                                    },
                                };
                                {
                                    let page = scraper::Html::parse_document(&text);
                                    for link in page.select(&scraper::Selector::parse("a").unwrap()) {
                                        let href = match link.value().attr("href") {
                                            None => continue,
                                            Some(h) => h,
                                        };
                                        if !href.starts_with("https://") {
                                            continue;
                                        }
                                        hrefs.push(href.to_string());
                                    }
                                }
                                let r: Result<()> = Ok(());
                                r
                            }).await.unwrap();
                            ctx.cache_put(&log, &cache_key, &hrefs).await;
                            hrefs
                        },
                    };
                    for href in hrefs {
                        if ctx.maybe_add_url(&log, &href).await {
                            break;
                        };
                    }
                }));
            }
        }
    }
    for f in sub_pool {
        f.await.unwrap();
    }

    // another broken lifetime workaround
    let w: WorkingWeights = (*ctx.config.lock().unwrap()).clone();
    Ok(w.clone())
}
