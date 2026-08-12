use std::env;
use std::error::Error;
use std::path::PathBuf;

pub(crate) const DEFAULT_VPP_PATH: &str = r"voicepeak.vpp";
pub(crate) const DEFAULT_OUTPUT_DIR: &str = "dataset";
pub(crate) const DEFAULT_VARIANTS_PER_BLOCK: usize = 15;
pub(crate) const SPEED_VALUES: [f64; 5] = [0.75, 0.875, 1.0, 1.125, 1.25];

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) vpp_paths: Vec<PathBuf>,
    pub(crate) output_dir: PathBuf,
    pub(crate) variants_per_block: usize,
    pub(crate) max_blocks: Option<usize>,
    pub(crate) block_indices: Option<Vec<usize>>,
    pub(crate) strict: bool,
    pub(crate) dry_run: bool,
}

pub(crate) fn parse_args() -> Result<Config, Box<dyn Error>> {
    parse_args_from(env::args().skip(1))
}

pub(crate) fn parse_args_from<I>(args: I) -> Result<Config, Box<dyn Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut positional = Vec::new();
    let mut vpp_paths = Vec::new();
    let mut variants_per_block = DEFAULT_VARIANTS_PER_BLOCK;
    let mut max_blocks = None;
    let mut block_indices = None;
    let mut strict = false;
    let mut dry_run = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--vpp" => {
                let path = args.next().ok_or("--vpp requires a VPP path")?;
                vpp_paths.push(PathBuf::from(path));
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

    let output_dir = if vpp_paths.is_empty() {
        match positional.as_slice() {
            [] => {
                vpp_paths.push(PathBuf::from(DEFAULT_VPP_PATH));
                PathBuf::from(DEFAULT_OUTPUT_DIR)
            }
            [vpp] => {
                vpp_paths.push(vpp.clone());
                PathBuf::from(DEFAULT_OUTPUT_DIR)
            }
            [vpp, output] => {
                vpp_paths.push(vpp.clone());
                output.clone()
            }
            _ => {
                return Err(
                    "multiple VPP files require repeatable --vpp PATH options; positional syntax supports one VPP and one output directory"
                        .into(),
                );
            }
        }
    } else {
        if positional.len() > 1 {
            return Err(
                "with --vpp, specify at most one positional argument for the output directory"
                    .into(),
            );
        }
        positional
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_DIR))
    };
    if vpp_paths.is_empty() {
        return Err("at least one VPP path is required".into());
    }
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
        vpp_paths,
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
         Generates SBV2-compatible data converted directly from one or more VPP files plus VPP-conditioned VOICEPEAK audio.\n\n\
         Options:\n\
           --vpp PATH         Add a VPP input; repeat for multiple VPP files\n\
           --variants N       Total variants per block; evenly split by speed (default: {DEFAULT_VARIANTS_PER_BLOCK})\n\
           --max-blocks N      Process only the first N blocks of each VPP\n\
           --blocks LIST       Process selected zero-based block indices in each VPP, e.g. 0,14,79,99\n\
           --strict            Stop at the first synthesis or alignment error\n\
           --dry-run           Print the generation plan without launching VOICEPEAK\n\
         Defaults:\n\
           VPP_PATH    {DEFAULT_VPP_PATH}\n\
           OUTPUT_DIR  {DEFAULT_OUTPUT_DIR}\n\n\
         Multiple VPP example:\n\
           generate_voice_from_voicepeak.exe --vpp first.vpp --vpp second.vpp dataset-root"
    );
}
