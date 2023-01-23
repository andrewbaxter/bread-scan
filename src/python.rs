use std::{
    collections::HashMap,
    path::Path,
};
use reqwest::header::{
    self,
    HeaderValue,
};
use serde::Deserialize;
use slog::Logger;
use tokio::{
    task::JoinHandle,
    spawn,
};
use crate::{
    warn,
    o,
    common::{
        Context,
        maybe_read,
        norm_repo,
    },
    aes,
    bbl,
};

fn process_dep(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, dep: String) {
    if dep == "python" {
        return;
    }
    let log = log.new(o!(dep = dep.to_string()));
    let ctx = ctx.clone();
    pool.push(spawn(async move {
        match aes!({
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
                    .get(&format!("https://pypi.org/pypi/{}/json", dep))
                    .await?
                    .header(header::ACCEPT, HeaderValue::from_static("application/json"))
                    .send()
                    .await?
                    .json()
                    .await?;
            bbl!('search, {
                async fn grab_repo(log: &Logger, ctx: &Context, raw_url: &str) -> bool {
                    match aes!({
                        let url = match norm_repo(raw_url)? {
                            Some(u) => u,
                            None => return Ok(false),
                        };
                        ctx.add_url_canonicalize(log, &url).await;
                        Ok(true)
                    }).await {
                        Err(e) => {
                            warn!(log, "Error parsing dep project url", url = raw_url, err = e.to_string());
                            false
                        },
                        Ok(o) => {
                            o
                        },
                    }
                }

                if let Some(info) = resp.info {
                    if grab_repo(&log, &ctx, &info.project_url).await {
                        break 'search;
                    }
                    for url in info.project_urls.values() {
                        if grab_repo(&log, &ctx, url).await {
                            break 'search;
                        }
                    }
                }
                warn!(log, "No repo-ish url found in dep metadata");
            });
            Ok(())
        }).await {
            Ok(_) => { },
            Err(e) => {
                warn!(log, "Error processing dependency", err = format!("{:?}", e));
            },
        }
    }));
}

pub fn process_python_pyproject(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, path: &Path) {
    let project_path = path.join("pyproject.toml");
    let log = log.new(o!(file = project_path.to_string_lossy().to_string()));

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
            warn!(log, "Error loading dep file", err = e.to_string());
            return;
        },
        Ok(Some(r)) => r,
    }) {
        Err(e) => {
            warn!(log, "Error loading dep file", err = e.to_string());
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
