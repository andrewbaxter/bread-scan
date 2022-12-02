use std::{fs, io::ErrorKind, path::Path};

use bread_common::projectconfig::{self, VersionedProjectConfig, FILENAME};
use reqwest::Client;

use crate::aes;

pub const DEFAULT_WEIGHT: u32 = 100;

pub struct Context {
    pub out: projectconfig::latest::Config,
    pub hc: Client,
}

pub async fn process_bread(ctx: &mut Context, path: &Path) {
    match aes!({
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
    })
    .await
    {
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "Error processing submodule at [[{}]]: {}",
                path.to_string_lossy(),
                e
            );
        }
    }
}
