use std::{
    path::Path,
};
use anyhow::{
    Result,
    Context as _,
};
use reqwest::StatusCode;
use slog::Logger;
use tokio::{
    spawn,
    task::JoinHandle,
};
use crate::{
    common::{
        maybe_read,
        Context,
        norm_repo,
    },
    warn,
    o,
    aes,
};

fn load_pom(bytes: &[u8]) -> Result<(sxd_document::Package, sxd_xpath::Context<'static>)> {
    let package = sxd_document::parser::parse(String::from_utf8_lossy(bytes).as_ref())?;
    let mut xctx = sxd_xpath::Context::new();
    let ns = sxd_xpath::evaluate_xpath(&package.as_document(), "namespace-uri(/*)").unwrap().string();
    xctx.set_namespace("n", &ns);
    Ok((package, xctx))
}

fn try_load_pom(path: &Path) -> Result<Option<(sxd_document::Package, sxd_xpath::Context)>> {
    let bytes = match maybe_read(&path) {
        Ok(None) => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(Some(r)) => r,
    };
    load_pom(&bytes).map(|p| Some(p))
}

fn process_dep(
    log: &Logger,
    ctx: &Context,
    pool: &mut Vec<JoinHandle<()>>,
    dep_group: String,
    dep_name: String,
    dep_ver: String,
) {
    let log = log.new(o!(dep_group = dep_group.clone(), dep_name = dep_name.clone(), dep_ver = dep_ver.clone()));
    let ctx = ctx.clone();
    pool.push(spawn(async move {
        match aes!({
            let url =
                format!(
                    "https://search.maven.org/remotecontent?filepath={group}/{name}/{ver}/{name}-{ver}.pom",
                    group = dep_group.replace('.', "/"),
                    name = dep_name,
                    ver = dep_ver
                );

            fn work_around_rust_lifetime_bugs(resp_bytes: &[u8]) -> Result<String> {
                let xpath =
                    sxd_xpath::Factory::new().build("normalize-space(//n:scm/n:url/text())").unwrap().unwrap();
                let (pom, xctx) = load_pom(resp_bytes).context("Error parsing dependency pom.xml")?;
                Ok(xpath.evaluate(&xctx, pom.as_document().root()).unwrap().string())
            }

            let resp = ctx.get(&url).await?.send().await?;
            if resp.status() == StatusCode::NOT_FOUND {
                return Ok(());
            }
            let body = resp.bytes().await?.to_vec();
            let repo =
                work_around_rust_lifetime_bugs(
                    &body,
                ).with_context(
                    || format!(
                        "Failed to extract pom from GET to {} with body:\n[[{}]]",
                        url,
                        String::from_utf8_lossy(&body)
                    ),
                )?;
            if !repo.is_empty() {
                ctx.add_url_canonicalize(&log, &norm_repo(&repo)?.unwrap_or(repo)).await;
            }
            Ok(())
        }).await {
            Ok(_) => { },
            Err(e) => {
                warn!(log, "Error processing dependency", err = format!("{:?}", e));
            },
        }
    }));
}

pub fn process_java_pom(base_log: &Logger, ctx: &Context, pool: &mut Vec<JoinHandle<()>>, base_path: &Path) {
    let path = base_path.join("pom.xml");
    let log = base_log.new(o!(file = path.to_string_lossy().to_string()));
    let (pom, xctx) = match try_load_pom(&path) {
        Ok(None) => return,
        Ok(Some(p)) => p,
        Err(e) => {
            warn!(log, "Error loading manifest", err = e.to_string());
            return;
        },
    };
    let factory = sxd_xpath::Factory::new();
    let xpath_group = factory.build("normalize-space(./n:groupId/text())").unwrap().unwrap();
    let xpath_name = factory.build("normalize-space(./n:artifactId/text())").unwrap().unwrap();
    let xpath_ver = factory.build("normalize-space(./n:version/text())").unwrap().unwrap();
    for xpath in [".//n:dependency", ".//n:extension", ".//n:plugin"] {
        let xpath_dep = factory.build(xpath).unwrap().unwrap();
        match xpath_dep.evaluate(&xctx, pom.as_document().root()).unwrap() {
            sxd_xpath::Value::Nodeset(nodes) => {
                for node in nodes {
                    let group = xpath_group.evaluate(&xctx, node).unwrap().string();
                    let name = xpath_name.evaluate(&xctx, node).unwrap().string();
                    let ver = xpath_ver.evaluate(&xctx, node).unwrap().string();
                    process_dep(&log, ctx, pool, group, name, ver);
                }
            },
            _ => { },
        }
    }
    let xpath_modules = factory.build("//n:modules/n:module").unwrap().unwrap();
    let xpath_text = factory.build("normalize-space(./text())").unwrap().unwrap();
    match xpath_modules.evaluate(&xctx, pom.as_document().root()).unwrap() {
        sxd_xpath::Value::Nodeset(nodes) => {
            for node in nodes {
                let child = xpath_text.evaluate(&xctx, node).unwrap().string();
                process_java_pom(base_log, ctx, pool, &base_path.join(child));
            }
        },
        _ => { },
    }
}
