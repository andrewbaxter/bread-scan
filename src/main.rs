use anyhow::{Context as _AnyhowContext, Result};
use bread_common::projectconfig::{self, VersionedProjectConfig, FILENAME};
use cargo_manifest::{Dependency, Manifest};
use reqwest::{
    header::{self, HeaderValue},
    Client,
};
use serde::Deserialize;
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::exit,
    str::FromStr,
};

pub mod flowcommon;

const DEFAULT_WEIGHT: u32 = 100;

struct Context {
    out: projectconfig::latest::Config,
    hc: Client,
}

impl Context {}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    match aes!({
        let mut ctx = Context {
            out: projectconfig::latest::Config {
                disabled: false,
                weights: projectconfig::v1::Weights {
                    accounts: Default::default(),
                    projects: Default::default(),
                },
            },
            hc: reqwest::Client::builder()
                .user_agent("https://github.com/andrewbaxter/bread-scan")
                .default_headers(
                    [(header::ACCEPT, HeaderValue::from_static("application/json"))]
                        .into_iter()
                        .collect(),
                )
                .build()?,
        };

        fn process_bread(ctx: &mut Context, path: &Path) -> Result<()> {
            let config: VersionedProjectConfig =
                serde_yaml::from_slice(&match fs::read(path.join(FILENAME)) {
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                    Ok(b) => b,
                })?;
            match config {
                VersionedProjectConfig::V1(config) => {
                    ctx.out.weights.accounts.extend(config.weights.accounts);
                    ctx.out.weights.projects.extend(config.weights.projects);
                }
            };
            Ok(())
        }

        async fn process_cargo_dep(ctx: &mut Context, id: String, dep: &Dependency) -> Result<()> {
            aes!({
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
                            process_bread(ctx, &PathBuf::from_str(&path)?)?;
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
            .with_context(|| format!("Error processing Cargo dep {}", id))
        }
        match Manifest::from_path("Cargo.toml") {
            Ok(m) => {
                for d in m.dependencies.unwrap_or_default() {
                    process_cargo_dep(&mut ctx, d.0, &d.1).await?;
                }
                for d in m.build_dependencies.unwrap_or_default() {
                    process_cargo_dep(&mut ctx, d.0, &d.1).await?;
                }
                for d in m.dev_dependencies.unwrap_or_default() {
                    process_cargo_dep(&mut ctx, d.0, &d.1).await?;
                }
                if let Some(t) = m.target {
                    for deps in t.into_values() {
                        for d in deps.dependencies {
                            process_cargo_dep(&mut ctx, d.0, &d.1).await?;
                        }
                        for d in deps.build_dependencies {
                            process_cargo_dep(&mut ctx, d.0, &d.1).await?;
                        }
                        for d in deps.dev_dependencies {
                            process_cargo_dep(&mut ctx, d.0, &d.1).await?;
                        }
                    }
                }
            }
            Err(cargo_manifest::Error::Io(e)) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => Err(e)?,
        };

        fs::write(FILENAME, serde_yaml::to_string(&ctx.out)?.as_bytes())?;

        Ok(())
    })
    .await
    {
        Ok(_) => {}
        Err(e) => {
            eprintln!("Fatal error scanning dependencies: {}", e);
            exit(1);
        }
    }
}
