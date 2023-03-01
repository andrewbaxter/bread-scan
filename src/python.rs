use std::{
    collections::HashMap,
    path::Path,
};
use reqwest::header::{
    self,
    HeaderValue,
};
use serde::Deserialize;
use slog::{
    Logger,
    warn,
    o,
};
use tokio::{
    task::JoinHandle,
    spawn,
};
use crate::{
    common::{
        Context,
        maybe_read,
    },
    aes,
};

fn process_dep(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, dep: String) {
    if dep == "python" {
        return;
    }
    let log = log.new(o!("dep" => dep.to_string()));
    let ctx = ctx.clone();
    pool.push(spawn(async move {
        match aes!({
            let cache_key = format!("python-{}", dep);
            let candidates = match ctx.cache_get::<Vec<String>>(&log, &cache_key).await {
                Some(c) => c,
                None => {
                    #[derive(Deserialize)]
                    pub struct Project {
                        pub info: Option<Info>,
                    }

                    #[derive(Deserialize)]
                    pub struct Info {
                        pub project_url: String,
                        pub project_urls: HashMap<String, String>,
                    }

                    let resp: Project =
                        ctx
                            .http_get(&format!("https://pypi.org/pypi/{}/json", dep))
                            .await?
                            .header(header::ACCEPT, HeaderValue::from_static("application/json"))
                            .send()
                            .await?
                            .json()
                            .await?;
                    let mut candidates = vec![];
                    if let Some(info) = resp.info {
                        candidates.push(info.project_url);
                        for url in info.project_urls.values() {
                            candidates.push(url.clone());
                        }
                    }
                    ctx.cache_put(&log, &cache_key, &candidates).await;
                    candidates
                },
            };
            for c in candidates {
                if ctx.maybe_add_url(&log, &c).await {
                    return Ok(());
                }
            }
            warn!(log, "No repo-ish url found in dep metadata");
            Ok(())
        }).await {
            Ok(_) => { },
            Err(e) => {
                warn!(
                    log,
                    "Error processing dependency";
                    "err" => #? e
                );
            },
        }
    }));
}

pub fn process_python_pyproject(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, path: &Path) {
    let project_path = path.join("pyproject.toml");
    let log = log.new(o!("file" => project_path.to_string_lossy().to_string()));

    #[derive(Deserialize)]
    struct Poetry {
        #[serde(rename = "dependencies")]
        poetry_deps: Option<HashMap<String, String>>,
        #[serde(rename = "dev-dependencies")]
        poetry_dev_deps: Option<HashMap<String, String>>,
    }

    #[derive(Deserialize)]
    struct Tool {
        poetry: Option<Poetry>,
    }

    #[derive(Deserialize)]
    struct PyProject {
        tool: Option<Tool>,
    }

    let proj = match toml::from_slice::<PyProject>(&match maybe_read(&project_path) {
        Ok(None) => {
            return;
        },
        Err(e) => {
            warn!(
                log,
                "Error loading dep file";
                "err" => #? e
            );
            return;
        },
        Ok(Some(r)) => r,
    }) {
        Err(e) => {
            warn!(
                log,
                "Error loading dep file";
                "err" => #? e
            );
            return;
        },
        Ok(b) => b,
    };
    if let Some(tool) = proj.tool {
        if let Some(poetry) = tool.poetry {
            for dep in poetry.poetry_deps.into_iter().map(|m| m.into_keys()).flatten() {
                process_dep(&log, ctx, pool, dep);
            }
            for dep in poetry.poetry_dev_deps.into_iter().map(|m| m.into_keys()).flatten() {
                process_dep(&log, ctx, pool, dep);
            }
        }
    }
}
