use serde::Deserialize;
use slog::Logger;
use anyhow::{
    Result,
    Context as _,
    anyhow,
};
use std::{
    fs,
    io::ErrorKind,
    path::Path,
    collections::HashMap,
};
use crate::{
    common::{
        Context,
        DEFAULT_WEIGHT,
    },
    o,
    warn,
    es,
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
    Ok(Some(serde_json::from_slice::<Package>(&match fs::read(&path) {
        Err(e) => {
            if e.kind() == ErrorKind::NotFound || e.raw_os_error().unwrap_or_default() == 20 {
                // 20 is NotADirectory, enum only on unstable (nop)
                return Ok(None);
            } else {
                return Err(e.into());
            }
        },
        Ok(r) => r,
    })?))
}

fn process_npm_dep(log: &Logger, ctx: &mut Context, root_path: &Path, dep: &str) {
    let log = log.new(o!(dep = dep.to_string()));
    match es!({
        let dep_path = root_path.join("node_modules").join(dep).join("package.json");
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
        ctx
            .config
            .lock()
            .unwrap()
            .weights
            .projects
            .insert(format!("https://{}{}", url.host_str().unwrap_or(""), path), DEFAULT_WEIGHT);
        Ok(())
    }) {
        Err(e) => {
            warn!(log, "Error processing dep", err = e.to_string());
        },
        Ok(_) => { },
    }
}

pub fn process_javascript_npm(log: &Logger, ctx: &mut Context, path: &Path) {
    let package_path = path.join("package.json");
    let log = log.new(o!(file = package_path.to_string_lossy().to_string()));
    let package = match try_load_packagejson(&package_path) {
        Err(e) => {
            warn!(log, "Error reading package.json", err = e.to_string());
            return;
        },
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    for dep in package.dependencies.iter().map(|m| m.keys()).flatten() {
        process_npm_dep(&log, ctx, path, dep);
    }
    for dep in package.dev_dependencies.iter().map(|m| m.keys()).flatten() {
        process_npm_dep(&log, ctx, path, dep);
    }
}
