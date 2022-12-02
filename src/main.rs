use anyhow::anyhow;
use bread_common::projectconfig::{self, VersionedProjectConfig, FILENAME};
use clap::Parser;
use reqwest::header::{self, HeaderValue};
use sloggers::{
    terminal::{Destination, TerminalLoggerBuilder},
    types::Severity,
    Build,
};
use std::{env::current_dir, fs, path::PathBuf, process::exit};

use crate::{common::Context, golang::process_golang_gomod, rust::process_rust_cargo};

pub mod common;
pub mod flowextra;
pub mod golang;
pub mod rust;
pub mod slogextra;

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(help = "Paths to scan for dependency files. If not specified, uses current directory.")]
    paths: Vec<PathBuf>,
    #[arg(short, long, help = "Write output to .bread.yml instead of stdout")]
    write: bool,
    #[arg(
        short,
        long,
        help = "Overwrite existing .bread.yml if it already exists"
    )]
    force: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut builder = TerminalLoggerBuilder::new();
    builder.level(Severity::Debug);
    builder.destination(Destination::Stderr);
    let log = builder.build().unwrap();

    match aes!({
        let args = Args::parse();
        let cwd = current_dir()?.canonicalize()?;
        let mut paths = vec![];
        for p in args.paths {
            paths.push(p.canonicalize()?);
        }
        if paths.is_empty() {
            paths.push(cwd.clone());
        }

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

        for path in paths {
            let log = log.new(o!(dir = path.to_string_lossy().to_string()));
            process_rust_cargo(&log, &mut ctx, &path).await;
            process_golang_gomod(&log, &mut ctx, &path).await;
        }

        let text = serde_yaml::to_string(&VersionedProjectConfig::V1(ctx.out))?;
        if args.write {
            let dest = cwd.join(FILENAME);
            if dest.exists() && !args.force {
                return Err(anyhow!(
                    "File already exists at {}. If you wish to overwrite it, specify --force",
                    dest.to_string_lossy(),
                ));
            }
            fs::write(&dest, text.as_bytes())?;
            info!(
                log,
                "Wrote new manifest",
                out = dest.to_string_lossy().to_string()
            );
            Ok(None)
        } else {
            Ok(Some(text))
        }
    })
    .await
    {
        Ok(Some(text)) => {
            drop(log);
            println!("{}", text);
        }
        Ok(None) => {
            drop(log);
        }
        Err(e) => {
            err!(
                log,
                "Fatal error encountered while scanning dependencies",
                err = format!("{:?}", e)
            );
            drop(log);
            exit(1);
        }
    }
}
