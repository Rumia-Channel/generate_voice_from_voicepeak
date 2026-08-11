use std::env;
use std::error::Error;
use std::path::PathBuf;

pub(crate) const DEFAULT_VPP_PATH: &str = r"voicepeak.vpp";
pub(crate) const DEFAULT_OUTPUT_DIR: &str = "dataset";
pub(crate) const DEFAULT_VARIANTS_PER_BLOCK: usize = 15;
pub(crate) const SPEED_VALUES: [f64; 5] = [0.75, 0.875, 1.0, 1.125, 1.25];

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) vpp_path: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) variants_per_block: usize,
    pub(crate) max_blocks: Option<usize>,
    pub(crate) block_indices: Option<Vec<usize>>,
    pub(crate) strict: bool,
    pub(crate) dry_run: bool,
}

pub(crate) fn parse_args() -> Result<Config, Box<dyn Error>> {
    let mut positional = Vec::new();
    let mut variants_per_block = DEFAULT_VARIANTS_PER_BLOCK;
    let mut max_blocks = None;
    let mut block_indices = None;
    let mut strict = false;
    let mut dry_run = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--variants" => {
                variants_per_block = args
                    .next()
                    .ok_or("--variants requires a positive integer")?
                    .parse()?;
                if variants_per_block == 0 {
                    return Err("--variants must be greater than zero".into());
                }
            }
            "--max-blocks" => {
                let value: usize = args
                    .next()
                    .ok_or("--max-blocks requires a positive integer")?
                    .parse()?;
                if value == 0 {
                    return Err("--max-blocks must be greater than zero".into());
                }
                max_blocks = Some(value);
            }
            "--blocks" => {
                let value = args
                    .next()
                    .ok_or("--blocks requires comma-separated zero-based indices")?;
                let indices = value
                    .split(',')
                    .map(|item| item.parse::<usize>())
                    .collect::<Result<Vec<_>, _>>()?;
                if indices.is_empty() || indices.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err("--blocks must contain unique zero-based indices".into());
                }
                block_indices = Some(indices);
            }
            "--strict" => strict = true,
            "--dry-run" => dry_run = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown option: {value}").into());
            }
            value => positional.push(PathBuf::from(value)),
        }
    }

    let vpp_path = positional
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_VPP_PATH));
    let output_dir = positional
        .get(1)
        .cloned()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_DIR));
    if max_blocks.is_some() && block_indices.is_some() {
        return Err("--blocks cannot be combined with --max-blocks".into());
    }
    if !variants_per_block.is_multiple_of(SPEED_VALUES.len()) {
        return Err(format!(
            "--variants must be divisible by {} for balanced speed folders",
            SPEED_VALUES.len()
        )
        .into());
    }

    Ok(Config {
        vpp_path,
        output_dir,
        variants_per_block,
        max_blocks,
        block_indices,
        strict,
        dry_run,
    })
}

pub(crate) fn print_help() {
    println!(
        "Usage: generate_voice_from_voicepeak [VPP_PATH] [OUTPUT_DIR] [OPTIONS]\n\n\
         Generates SBV2-compatible data converted directly from VPP plus VPP-conditioned VOICEPEAK audio.\n\n\
         Options:\n\
           --variants N       Total variants per block; evenly split by speed (default: {DEFAULT_VARIANTS_PER_BLOCK})
           --max-blocks N      Process only the first N blocks
           --blocks LIST       Process selected zero-based block indices, e.g. 0,14,79,99
           --strict            Stop at the first synthesis or alignment error
           --dry-run           Print the generation plan without launching VOICEPEAK
         Defaults:\n\
           VPP_PATH    {DEFAULT_VPP_PATH}\n\
           OUTPUT_DIR  {DEFAULT_OUTPUT_DIR}"
    );
}
