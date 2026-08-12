mod config;
mod generator;
mod julius;
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
        for vpp_path in &config.vpp_paths {
            let project = ProjectFile::from_path(vpp_path)?;
            generator::print_plan(vpp_path, &project, &config);
        }
        return Ok(());
    }

    generator::generate_dataset(&config)
}

#[cfg(test)]
mod tests;
