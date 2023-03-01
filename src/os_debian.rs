use std::fs;
use anyhow::{
    Result,
    anyhow,
};
use slog::{
    Logger,
    warn,
    o,
};
use tokio::{
    process::Command,
    spawn,
};
use crate::{
    common::{
        Context,
        Supercontext,
        WorkingWeights,
        maybe_read,
    },
    aes,
};

pub async fn process(log: Logger, supercontext: Supercontext) -> Result<WorkingWeights> {
    let ctx = Context::new(supercontext.clone());
    let manual = Command::new("apt-mark").arg("showmanual").output().await?;
    if !manual.status.success() {
        return Err(anyhow!("Listing manually installed packages failed:\n{:?}", manual));
    }
    let mut sub_pool = vec![];
    for package in String::from_utf8_lossy(&manual.stdout).lines() {
        let package = package.to_string();
        let ctx = ctx.clone();
        let log = log.new(o!("package" => package.clone()));
        sub_pool.push(spawn(async move {
            match aes!({
                let cache_key = format!("debian-{}", package);
                let source: Option<String> = match ctx.cache_get::<Option<String>>(&log, &cache_key).await {
                    Some(s) => s,
                    None => {
                        let source = aes!({
                            let pkg_info = Command::new("dpkg").arg("-s").arg(&package).output().await?;
                            if !pkg_info.status.success() {
                                warn!(
                                    log,
                                    "Getting package info failed";
                                    "output" => #? pkg_info
                                );
                                return Ok(None);
                            }
                            let mut ver = None;
                            for line in String::from_utf8_lossy(&pkg_info.stdout).lines() {
                                match line.strip_prefix("Version: ") {
                                    Some(v) => ver = Some(v.to_string()),
                                    None => continue,
                                };
                            }
                            let ver = match ver {
                                Some(v) => v,
                                None => {
                                    warn!(log, "Unable to determine version for package");
                                    return Ok(None);
                                },
                            };
                            let source_dir = tempfile::tempdir()?;
                            let res =
                                Command::new("apt-get")
                                    .arg("source")
                                    .arg(format!("{}={}", package, ver))
                                    .current_dir(source_dir.path())
                                    .output()
                                    .await?;
                            if !res.status.success() {
                                warn!(log, "Unable to get source for package");
                                return Ok(None);
                            }
                            let copyright =
                                match maybe_read(
                                    &fs::read_dir(source_dir.path())?
                                        .next()
                                        .ok_or_else(
                                            || anyhow!(
                                                "Did apt-get source for {} but no source directory created",
                                                package
                                            ),
                                        )??
                                        .path()
                                        .join("debian/copyright"),
                                )? {
                                    Some(c) => c,
                                    None => {
                                        return Ok(None);
                                    },
                                };
                            for line in String::from_utf8_lossy(&copyright).lines() {
                                if let Some(source) = line.strip_prefix("Source: ") {
                                    return Ok(Some(source.to_string()));
                                }
                            }
                            return Ok(None);
                        }).await?;
                        ctx.cache_put(&log, &cache_key, &source).await;
                        source
                    },
                };
                if let Some(source) = source {
                    ctx.maybe_add_url(&log, &source).await;
                }
                Ok(())
            }).await {
                Ok(_) => { },
                Err(e) => {
                    warn!(
                        log,
                        "Error looking up repo for package";
                        "err" => #? e
                    );
                },
            }
        }));
    }
    for f in sub_pool {
        f.await.unwrap();
    }

    // another broken lifetime workaround
    let w: WorkingWeights = (*ctx.config.lock().unwrap()).clone();
    Ok(w.clone())
}
