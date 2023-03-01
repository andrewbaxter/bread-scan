use serde::Deserialize;
use slog::{
    Logger,
    o,
    warn,
};
use anyhow::{
    Result,
    Context as _,
    anyhow,
};
use tokio::{
    task::JoinHandle,
    spawn,
};
use std::{
    path::Path,
    collections::HashMap,
};
use crate::{
    common::{
        Context,
        maybe_read,
    },
    aes,
};

#[derive(Deserialize)]
struct PackageRepository {
    url: String,
}

#[derive(Deserialize)]
struct Package {
    dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: Option<HashMap<String, String>>,
    repository: Option<PackageRepository>,
}

fn try_load_packagejson(path: &Path) -> Result<Option<Package>> {
    Ok(Some(serde_json::from_slice::<Package>(&match maybe_read(&path) {
        Ok(None) => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(Some(r)) => r,
    })?))
}

fn process_npm_dep(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, root_path: &Path, dep: &str) {
    let log = log.new(o!("dep" => dep.to_string()));
    let ctx = ctx.clone();
    let dep_path = root_path.join("node_modules").join(dep).join("package.json");
    pool.push(spawn(async move {
        match aes!({
            let package = match try_load_packagejson(&dep_path).context("Error loading package.json")? {
                None => {
                    return Err(anyhow!("NPM package missing in node_modules"));
                },
                Some(p) => p,
            };
            let repo = match package.repository {
                None => return Ok(()),
                Some(r) => r,
            };
            let url = url::Url::parse(&repo.url).context("Unparsable repo url")?;
            let path = url.path().rsplitn(2, ".git").collect::<Vec<&str>>().last().unwrap().to_string();
            ctx.add_url_canonicalize(&log, &format!("https://{}{}", url.host_str().unwrap_or(""), path)).await;
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

pub fn process_javascript_npm(log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, path: &Path) {
    let package_path = path.join("package.json");
    let log = log.new(o!("file" => package_path.to_string_lossy().to_string()));
    let package = match try_load_packagejson(&package_path) {
        Err(e) => {
            warn!(
                log,
                "Error reading package.json";
                "err" => #? e
            );
            return;
        },
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    for dep in package.dependencies.iter().map(|m| m.keys()).flatten() {
        process_npm_dep(&log, ctx, pool, path, dep);
    }
    for dep in package.dev_dependencies.iter().map(|m| m.keys()).flatten() {
        process_npm_dep(&log, ctx, pool, path, dep);
    }
}
