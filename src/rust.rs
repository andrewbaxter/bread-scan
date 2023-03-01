use cargo_manifest::{
    Dependency,
    Manifest,
};
use reqwest::header::{
    self,
    HeaderValue,
};
use serde::Deserialize;
use slog::{
    Logger,
    warn,
};
use tokio::{
    spawn,
    task::JoinHandle,
};
use std::{
    io::ErrorKind,
    path::{
        Path,
    },
};
use crate::{
    aes,
    common::{
        Context,
    },
    o,
};

fn process_dep(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, id: String, dep: &Dependency) {
    let log = log.new(o!("dependency" => id.clone()));
    let ctx = ctx.clone();
    let dep = dep.clone();
    pool.push(spawn(async move {
        match aes!({
            let id = match dep {
                Dependency::Simple(_) => id.clone(),
                Dependency::Detailed(d) => {
                    let mut id = id.clone();
                    if let Some(git) = &d.git {
                        ctx.config.lock().unwrap().projects.insert(git.to_string(), None);
                        return Ok(());
                    }
                    if d.path.is_some() {
                        return Ok(());
                    }
                    if let Some(pkg) = &d.package {
                        id = pkg.to_string();
                    }
                    id
                },
            };
            let cache_key = format!("rust-{}", id);
            let repo = match ctx.cache_get(&log, &cache_key).await {
                Some(r) => r,
                None => {
                    #[derive(Deserialize)]
                    struct CratesRespCrate {
                        repository: Option<String>,
                    }

                    #[derive(Deserialize)]
                    struct CratesResp {
                        #[serde(rename = "crate")]
                        crate_: CratesRespCrate,
                    }

                    let resp: CratesResp =
                        ctx
                            .http_get(&format!("https://crates.io/api/v1/crates/{}", id))
                            .await?
                            .header(header::ACCEPT, HeaderValue::from_static("application/json"))
                            .send()
                            .await?
                            .json()
                            .await?;
                    let out = resp.crate_.repository;
                    ctx.cache_put(&log, &cache_key, &out).await;
                    out
                },
            };
            if let Some(repo) = repo {
                ctx.add_url(&repo).await;
            }
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

pub fn process_rust_cargo(base_log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, base_path: &Path) {
    let path = base_path.join("Cargo.toml");
    let log = base_log.new(o!("file" => path.to_string_lossy().to_string()));
    let m = match Manifest::from_path(&path) {
        Ok(m) => m,
        Err(cargo_manifest::Error::Io(e)) if e.kind() == ErrorKind::NotFound => {
            return;
        },
        Err(e) => {
            warn!(
                log,
                "Error loading manifest";
                "err" => #? e
            );
            return;
        },
    };
    for d in m.dependencies.unwrap_or_default() {
        process_dep(&log, ctx, pool, d.0, &d.1);
    }
    for d in m.build_dependencies.unwrap_or_default() {
        process_dep(&log, ctx, pool, d.0, &d.1);
    }
    for d in m.dev_dependencies.unwrap_or_default() {
        process_dep(&log, ctx, pool, d.0, &d.1);
    }
    if let Some(t) = m.target {
        for deps in t.into_values() {
            for d in deps.dependencies {
                process_dep(&log, ctx, pool, d.0, &d.1);
            }
            for d in deps.build_dependencies {
                process_dep(&log, ctx, pool, d.0, &d.1);
            }
            for d in deps.dev_dependencies {
                process_dep(&log, ctx, pool, d.0, &d.1);
            }
        }
    }
    for member in m.workspace.iter().map(|w| &w.members).flatten() {
        process_rust_cargo(&base_log, ctx, pool, &base_path.join(member));
    }
}
