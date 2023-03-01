use anyhow::{
    anyhow,
    Context as _,
    Result,
};
use bread_common::{
    projectconfig::{
        VersionedProjectConfig,
        FILENAME,
        self,
    },
    accountconfig,
};
use clap::{
    Parser,
};
use common::{
    maybe_read,
    Context,
    WorkingWeights,
    DEFAULT_WEIGHT,
    USER_AGENT,
    WorkingAccount,
};
use java::process_java_pom;
use javascript::process_javascript_npm;
use path_absolutize::Absolutize;
use platform_dirs::AppDirs;
use python::process_python_pyproject;
use reqwest::header::{
    AUTHORIZATION,
    HeaderMap,
    HeaderValue,
};
use slog::{
    o,
    error,
};
use sloggers::{
    terminal::{
        Destination,
        TerminalLoggerBuilder,
    },
    types::{
        Severity,
    },
    Build,
};
use tokio::{
    spawn,
    task::JoinHandle,
};
use std::{
    env::{
        current_dir,
        self,
    },
    fs,
    path::PathBuf,
    process::exit,
    str::FromStr,
};
use crate::{
    golang::process_golang_gomod,
    rust::process_rust_cargo,
    common::Supercontext,
};

pub mod common;
pub mod flowextra;
pub mod golang;
pub mod javascript;
pub mod java;
pub mod python;
pub mod rust;
pub mod os_arch;
pub mod os_debian;

pub const ENV_BREAD_TOKEN: &'static str = "BREAD_TOKEN";

#[derive(Clone, Debug)]
pub enum Os {
    Debian,
    Arch,
}

#[derive(Clone, Debug)]
pub enum ArgSource {
    Project(PathBuf),
    Donate,
    Os(Os),
    File(PathBuf),
}

impl FromStr for ArgSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut splits = s.splitn(2, "=");
        match splits.next().unwrap() {
            "project" => Ok(
                ArgSource::Project(
                    PathBuf::from(splits.next().ok_or_else(|| anyhow!("Missing path"))?).absolutize()?.to_path_buf(),
                ),
            ),
            "os" => {
                match splits.next().ok_or_else(|| anyhow!("Missing OS name"))? {
                    "debian" => Ok(ArgSource::Os(Os::Debian)),
                    "arch" => Ok(ArgSource::Os(Os::Arch)),
                    o => Err(anyhow!("Unrecognized os [[{}]]", o)),
                }
            },
            "donate" => {
                if splits.next().is_some() {
                    return Err(anyhow!("Donate takes no parameters, but one specified"));
                }
                Ok(ArgSource::Donate)
            },
            "file" => Ok(
                ArgSource::File(
                    PathBuf::from(splits.next().ok_or_else(|| anyhow!("Missing path"))?).absolutize()?.to_path_buf(),
                ),
            ),
            o => {
                Err(anyhow!("Unknown source type [[{}]]", o))
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum ArgDest {
    ProjectYaml(PathBuf),
    Donate,
    File(PathBuf),
}

impl FromStr for ArgDest {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut splits = s.splitn(2, "=");
        match splits.next().unwrap() {
            "project-yaml" => Ok(
                ArgDest::ProjectYaml(
                    PathBuf::from(splits.next().ok_or_else(|| anyhow!("Missing path"))?).absolutize()?.to_path_buf(),
                ),
            ),
            "donate" => {
                if splits.next().is_some() {
                    return Err(anyhow!("donate takes no parameters, but one specified"));
                }
                Ok(ArgDest::Donate)
            },
            "file" => Ok(
                ArgDest::File(
                    PathBuf::from(splits.next().ok_or_else(|| anyhow!("Missing path"))?).absolutize()?.to_path_buf(),
                ),
            ),
            o => {
                Err(anyhow!("Unknown dest type [[{}]]", o))
            },
        }
    }
}

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(
        long,
        short = 's',
        help =
            "Where to search for donation targets; can be `project=PATH` where `PATH` is the project directory, `donate` which pulls your current donation targets from your account (need environment variable `BREAD_TOKEN`), `os=OS` scans your computer for installed software (supported OS's are debian, nixos, and arch), or `file=PATH` where `PATH` is the file generated by a previous invocation of bread-scan with dest `file=PATH`. Later sources override previous sources.",
    )]
    source: Vec<ArgSource>,
    #[arg(
        long,
        short = 'd',
        help =
            "Where to write the results; can be `project-yaml=PATH` where `PATH` is the project directory, `donate` which updates the current donations in your account, or `file=PATH` where `PATH` is a filename. If the destination exists, the results are merged (by default, new entries are added and existing entries are updated only).",
    )]
    dest: Vec<ArgDest>,
    #[arg(
        long,
        short = 'x',
        help = "Delete project entries at the destination if they weren't present in the scan results",
    )]
    remove: bool,
    #[arg(long, help = "Delete account entries at the destination if they weren't present in the scan results")]
    remove_accounts: bool,
}

fn api_client() -> Result<reqwest::Client> {
    let token =
        env::var(
            ENV_BREAD_TOKEN,
        ).with_context(|| format!("Failed to read environment variable {}", ENV_BREAD_TOKEN))?;
    Ok(
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(
                HeaderMap::from_iter(
                    [(AUTHORIZATION, HeaderValue::from_str(&format!("Basic {}", &token)).unwrap())].into_iter(),
                ),
            )
            .build()
            .unwrap(),
    )
}

async fn get_donate_weights(hc: &reqwest::Client) -> Result<accountconfig::v1::Weights> {
    let config: accountconfig::v1::Weights =
        hc.get("https://bre.ad/api/account_get_donate_weights").send().await?.json().await?;
    Ok(config)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut builder = TerminalLoggerBuilder::new();
    builder.level(Severity::Debug);
    builder.destination(Destination::Stderr);
    let log = builder.build().unwrap();
    match aes!({
        let mut args = Args::parse();
        let cwd = current_dir()?.canonicalize()?;
        let dirs = AppDirs::new(Some("bread-scan"), false).unwrap();
        let supercontext = Supercontext::new(dirs.cache_dir);
        if args.source.is_empty() {
            args.source.push(ArgSource::Project(cwd.clone()));
        }
        let mut pool: Vec<JoinHandle<Result<WorkingWeights, anyhow::Error>>> = vec![];
        for source in args.source {
            match source {
                ArgSource::Project(p) => {
                    let ctx = Context::new(supercontext.clone());
                    let log = log.new(o!("dir" => p.to_string_lossy().to_string()));
                    let mut sub_pool = vec![];
                    process_rust_cargo(&log, &ctx, &mut sub_pool, &p);
                    process_golang_gomod(&log, &ctx, &p);
                    process_javascript_npm(&log, &ctx, &mut sub_pool, &p);
                    process_python_pyproject(&log, &ctx, &mut sub_pool, &p);
                    process_java_pom(&log, &ctx, &mut sub_pool, &p);
                    pool.push(spawn(async move {
                        for f in sub_pool {
                            f.await.unwrap();
                        }
                        Ok(ctx.config.lock().unwrap().clone())
                    }));
                },
                ArgSource::Donate => {
                    let hc = api_client()?;
                    pool.push(spawn(async move {
                        let config = get_donate_weights(&hc).await?;
                        Ok(WorkingWeights {
                            accounts: config.accounts.into_iter().map(|(k, v)| (k, WorkingAccount {
                                memo: v.memo,
                                weight: Some(v.weight),
                            })).collect(),
                            projects: config.projects.into_iter().map(|(k, v)| (k, Some(v))).collect(),
                        })
                    }));
                },
                ArgSource::Os(o) => {
                    let log = log.new(o!("os" => format !("{:?}", o)));
                    match o {
                        Os::Debian => pool.push(spawn(os_debian::process(log, supercontext.clone()))),
                        Os::Arch => pool.push(spawn(os_arch::process(log, supercontext.clone()))),
                    }
                },
                ArgSource::File(p) => {
                    let f: WorkingWeights = serde_json::from_slice(&fs::read(p)?)?;
                    pool.push(spawn(async {
                        Ok(f)
                    }));
                },
            }
        }
        let mut working = WorkingWeights::default();
        for f in pool {
            let w = f.await.unwrap()?;
            working.accounts.extend(w.accounts);
            working.projects.extend(w.projects);
        }
        if args.dest.is_empty() {
            args.dest.push(ArgDest::ProjectYaml(cwd.clone()));
        }
        for dest in args.dest {
            match dest {
                ArgDest::ProjectYaml(p) => {
                    let manifest_path = p.join(FILENAME);
                    let mut config =
                        maybe_read(&manifest_path)
                            .and_then(|r| match r {
                                Some(b) => Ok(Some(match serde_yaml::from_slice::<VersionedProjectConfig>(&b)? {
                                    VersionedProjectConfig::V1(v) => v,
                                })),
                                None => Ok(None),
                            })?
                            .unwrap_or_else(
                                || projectconfig::v1::Config { weights: projectconfig::v1::Weights::default() },
                            );
                    for (a, v) in &working.accounts {
                        match config.weights.accounts.entry(*a) {
                            std::collections::hash_map::Entry::Occupied(mut e) => if let Some(v) = &v.weight {
                                *e.get_mut() = *v;
                            },
                            std::collections::hash_map::Entry::Vacant(e) => {
                                e.insert(v.weight.unwrap_or(DEFAULT_WEIGHT));
                            },
                        }
                    }
                    for (p, v) in &working.projects {
                        match config.weights.projects.entry(p.clone()) {
                            std::collections::hash_map::Entry::Occupied(mut e) => if let Some(v) = v {
                                *e.get_mut() = *v;
                            },
                            std::collections::hash_map::Entry::Vacant(e) => {
                                e.insert(v.unwrap_or(DEFAULT_WEIGHT));
                            },
                        }
                    }
                    if args.remove_accounts {
                        config.weights.accounts.retain(|k, _| working.accounts.contains_key(k));
                    }
                    if args.remove {
                        config.weights.projects.retain(|k, _| working.projects.contains_key(k));
                    }
                    fs::write(
                        &manifest_path,
                        serde_yaml::to_string(&VersionedProjectConfig::V1(config))?.as_bytes(),
                    ).context("failed to write project yaml")?;
                },
                ArgDest::Donate => {
                    let hc = api_client()?;
                    let mut config = get_donate_weights(&hc).await?;
                    for (a, v) in &working.accounts {
                        match config.accounts.entry(*a) {
                            std::collections::hash_map::Entry::Occupied(mut e) => {
                                if let Some(v) = &v.weight {
                                    e.get_mut().weight = *v;
                                }
                                if !v.memo.is_empty() {
                                    e.get_mut().memo = v.memo.clone();
                                }
                            },
                            std::collections::hash_map::Entry::Vacant(e) => {
                                e.insert(accountconfig::v1::AccountDest {
                                    weight: v.weight.unwrap_or(DEFAULT_WEIGHT),
                                    memo: v.memo.clone(),
                                });
                            },
                        }
                    }
                    for (p, v) in &working.projects {
                        match config.projects.entry(p.clone()) {
                            std::collections::hash_map::Entry::Occupied(mut e) => if let Some(v) = v {
                                *e.get_mut() = *v;
                            },
                            std::collections::hash_map::Entry::Vacant(e) => {
                                e.insert(v.unwrap_or(DEFAULT_WEIGHT));
                            },
                        }
                    }
                    if args.remove_accounts {
                        config.accounts.retain(|k, _| working.accounts.contains_key(k));
                    }
                    if args.remove {
                        config.projects.retain(|k, _| working.projects.contains_key(k));
                    }
                    hc.post("https://bre.ad/api/account_set_donate_weights").json(&config).send().await?;
                },
                ArgDest::File(p) => {
                    fs::write(
                        &p,
                        serde_json::to_string_pretty(&working)?.as_bytes(),
                    ).context("failed to write to file destination")?;
                },
            }
        }
        Ok(())
    }).await {
        Ok(_) => {
            drop(log);
        },
        Err(e) => {
            eprintln!("err {:?}", e);
            error!(
                log,
                "Exiting with fatal error";
                "err" => #? e
            );
            drop(log);
            exit(1);
        },
    }
}
