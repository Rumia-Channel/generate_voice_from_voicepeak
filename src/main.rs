mod config;
mod generator;
mod mfa;
mod models;
mod output;
mod sbv2;
mod util;
mod voicepeak;

use std::error::Error;

use vpsdk::vpp::ProjectFile;

fn main() -> Result<(), Box<dyn Error>> {
    let config = config::parse_args()?;
    if config.dry_run {
        let project = ProjectFile::from_path(&config.vpp_path)?;
        generator::print_plan(&project, &config);
        return Ok(());
    }

    generator::generate_dataset(&config)
}

#[cfg(test)]
mod tests;
