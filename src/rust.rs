use cargo_manifest::{Dependency, Manifest};
use serde::Deserialize;
use slog::Logger;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::{
    aes,
    common::{process_bread, Context, DEFAULT_WEIGHT},
    o, warn,
};

async fn process_cargo_dep(log: &Logger, ctx: &mut Context, id: String, dep: &Dependency) {
    let log = log.new(o!(dependency = id.clone()));
    match aes!({
        let id = match dep {
            Dependency::Simple(_) => id.clone(),
            Dependency::Detailed(d) => {
                let mut id = id.clone();
                if let Some(git) = &d.git {
                    ctx.out
                        .weights
                        .projects
                        .insert(git.to_string(), DEFAULT_WEIGHT);
                    return Ok(());
                }
                if let Some(path) = &d.path {
                    process_bread(ctx, &PathBuf::from_str(&path)?).await;
                    return Ok(());
                }
                if let Some(pkg) = &d.package {
                    id = pkg.to_string();
                }
                id
            }
        };
        #[derive(Deserialize)]
        struct CratesRespCrate {
            repository: Option<String>,
        }
        #[derive(Deserialize)]
        struct CratesResp {
            #[serde(rename = "crate")]
            crate_: CratesRespCrate,
        }
        let resp: CratesResp = ctx
            .hc
            .get(format!("https://crates.io/api/v1/crates/{}", id))
            .send()
            .await?
            .json()
            .await?;
        if let Some(repo) = resp.crate_.repository {
            ctx.out.weights.projects.insert(repo, DEFAULT_WEIGHT);
        }
        Ok(())
    })
    .await
    {
        Ok(_) => {}
        Err(e) => {
            warn!(log, "Error processing dependency", err = format!("{:?}", e));
        }
    }
}

pub async fn process_rust_cargo(log: &Logger, ctx: &mut Context, path: &Path) {
    let path = path.join("Cargo.toml");
    let log = log.new(o!(file = path.to_string_lossy().to_string()));
    match Manifest::from_path(&path) {
        Ok(m) => {
            for d in m.dependencies.unwrap_or_default() {
                process_cargo_dep(&log, &mut *ctx, d.0, &d.1).await;
            }
            for d in m.build_dependencies.unwrap_or_default() {
                process_cargo_dep(&log, &mut *ctx, d.0, &d.1).await;
            }
            for d in m.dev_dependencies.unwrap_or_default() {
                process_cargo_dep(&log, &mut *ctx, d.0, &d.1).await;
            }
            if let Some(t) = m.target {
                for deps in t.into_values() {
                    for d in deps.dependencies {
                        process_cargo_dep(&log, &mut *ctx, d.0, &d.1).await;
                    }
                    for d in deps.build_dependencies {
                        process_cargo_dep(&log, &mut *ctx, d.0, &d.1).await;
                    }
                    for d in deps.dev_dependencies {
                        process_cargo_dep(&log, &mut *ctx, d.0, &d.1).await;
                    }
                }
            }
        }
        Err(cargo_manifest::Error::Io(e)) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            warn!(log, "Error loading manifest", err = format!("{:?}", e));
        }
    };
}
