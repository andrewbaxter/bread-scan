use bread_common::projectconfig::{
    VersionedProjectConfig,
    FILENAME,
};
use clap::Parser;
use common::{
    maybe_read,
    Context,
};
use java::process_java_pom;
use javascript::process_javascript_npm;
use python::process_python_pyproject;
use sloggers::{
    terminal::{
        Destination,
        TerminalLoggerBuilder,
    },
    types::Severity,
    Build,
};
use std::{
    env::current_dir,
    fs,
    path::PathBuf,
    process::exit,
};
use crate::{
    golang::process_golang_gomod,
    rust::process_rust_cargo,
};

pub mod common;
pub mod flowextra;
pub mod golang;
pub mod javascript;
pub mod java;
pub mod python;
pub mod rust;
pub mod slogextra;

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(help = "Paths to scan for dependency files. If not specified, uses current directory.")]
    paths: Vec<PathBuf>,
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
        let manifest_path = cwd.join(FILENAME);
        let ctx = Context::new(maybe_read(&manifest_path).and_then(|r| match r {
            Some(b) => Ok(Some(match serde_yaml::from_slice::<VersionedProjectConfig>(&b)? {
                VersionedProjectConfig::V1(v) => v,
            })),
            None => Ok(None),
        })?);
        let mut pool = vec![];
        for path in paths {
            let log = log.new(o!(dir = path.to_string_lossy().to_string()));
            process_rust_cargo(&log, &ctx, &mut pool, &path);
            process_golang_gomod(&log, &ctx, &path);
            process_javascript_npm(&log, &ctx, &mut pool, &path);
            process_python_pyproject(&log, &ctx, &mut pool, &path);
            process_java_pom(&log, &ctx, &mut pool, &path);
        }
        for f in pool {
            f.await.unwrap();
        }
        fs::write(
            &manifest_path,
            serde_yaml::to_string(&VersionedProjectConfig::V1(ctx.config.lock().unwrap().to_owned()))?.as_bytes(),
        )?;
        Ok(())
    }).await {
        Ok(_) => {
            drop(log);
        },
        Err(e) => {
            err!(log, "Fatal error encountered while scanning dependencies", err = format!("{:?}", e));
            drop(log);
            exit(1);
        },
    }
}
