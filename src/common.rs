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
        let config: VersionedProjectConfig = serde_yaml::from_slice(&match fs::read(path.join(FILENAME)) {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Ok(());
            },
            Err(e) => {
                return Err(e.into());
            },
            Ok(b) => b,
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
