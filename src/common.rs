use std::{
    fs,
    io::ErrorKind,
    path::Path,
    sync::{
        Arc,
        Mutex,
    },
};
use bread_common::projectconfig::{
    self,
    VersionedProjectConfig,
    FILENAME,
};
use anyhow::Result;
use reqwest::Client;
use crate::aes;

pub const DEFAULT_WEIGHT: u32 = 100;

#[derive(Clone)]
pub struct Context {
    pub hc: Client,
    pub config: Arc<Mutex<projectconfig::latest::Config>>,
}

pub async fn process_bread(ctx: &Context, path: &Path) {
    match aes!({
        let config: VersionedProjectConfig = serde_yaml::from_slice(&match maybe_read(&path.join(FILENAME)) {
            Err(e) => {
                return Err(e.into());
            },
            Ok(None) => {
                return Ok(());
            },
            Ok(Some(b)) => b,
        })?;
        match config {
            VersionedProjectConfig::V1(config) => {
                let mut write = ctx.config.lock().unwrap();
                write.weights.accounts.extend(config.weights.accounts);
                write.weights.projects.extend(config.weights.projects);
            },
        };
        Ok(())
    }).await {
        Ok(_) => { },
        Err(e) => {
            eprintln!("Error processing submodule at [[{}]]: {}", path.to_string_lossy(), e);
        },
    }
}

pub fn maybe_read(p: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(&p) {
        Err(e) => {
            if e.kind() == ErrorKind::NotFound || e.raw_os_error().unwrap_or_default() == 20 {
                // 20 is NotADirectory, enum only on unstable (nop)
                return Ok(None);
            } else {
                return Err(e.into());
            }
        },
        Ok(r) => return Ok(Some(r)),
    }
}
