use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::config::SPEED_VALUES;

pub(crate) struct SpeedWriter {
    pub(crate) speed: f64,
    pub(crate) root: PathBuf,
    pub(crate) labels: BufWriter<File>,
    pub(crate) sbv2_labels: BufWriter<File>,
    pub(crate) requests: BufWriter<File>,
    pub(crate) rejects: BufWriter<File>,
    pub(crate) generated: usize,
    pub(crate) failed: usize,
}

impl SpeedWriter {
    pub(crate) fn new(output_dir: &Path, speed: f64) -> Result<Self, Box<dyn Error>> {
        let root = output_dir.join(format!("speed_{speed:.3}"));
        let wav_dir = root.join("wav");
        let julius_dir = root.join("julius");
        fs::create_dir_all(&wav_dir)?;
        fs::create_dir_all(&julius_dir)?;
        clear_generated_files(&wav_dir, &["wav", "lab"])?;
        clear_generated_files(&julius_dir, &["wav", "txt", "lab", "log", "dfa", "dict"])?;
        Ok(Self {
            speed,
            labels: BufWriter::new(File::create(root.join("labels.jsonl"))?),
            sbv2_labels: BufWriter::new(File::create(root.join("metadata.sbv2.jsonl"))?),
            requests: BufWriter::new(File::create(root.join("requests.jsonl"))?),
            rejects: BufWriter::new(File::create(root.join("rejects.jsonl"))?),
            root,
            generated: 0,
            failed: 0,
        })
    }
}

fn clear_generated_files(directory: &Path, extensions: &[&str]) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let extension = path.extension().and_then(|value| value.to_str());
        if extension.is_some_and(|extension| extensions.contains(&extension)) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub(crate) fn speed_group_index(speed: f64) -> Result<usize, Box<dyn Error>> {
    SPEED_VALUES
        .iter()
        .position(|candidate| (*candidate - speed).abs() < f64::EPSILON)
        .ok_or_else(|| format!("unsupported speed group: {speed}").into())
}

pub(crate) fn write_reject(
    writer: &mut BufWriter<File>,
    id: &str,
    stage: &str,
    error: &str,
) -> Result<(), Box<dyn Error>> {
    writeln!(
        writer,
        "{}",
        serde_json::to_string(&serde_json::json!({
            "id": id,
            "stage": stage,
            "error": error,
        }))?
    )?;
    Ok(())
}
