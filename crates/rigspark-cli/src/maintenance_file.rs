//! Safe read/replace/create of maintenance data files (catalogs, evidence, state).

use rigspark_runtime::secure_fs::Directory;
use std::{
    io,
    path::{Path, PathBuf},
};

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub struct File {
    directory: Directory,
    name: PathBuf,
}
impl File {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("file parent required"))?
            .canonicalize()?;
        let name = PathBuf::from(
            path.file_name()
                .ok_or_else(|| io::Error::other("filename required"))?,
        );
        Ok(Self {
            directory: Directory::open(&parent)?,
            name,
        })
    }
    pub fn read(&self) -> io::Result<Vec<u8>> {
        self.directory.read(&self.name, MAX_FILE_BYTES, false)
    }
    pub fn read_optional(&self) -> io::Result<Option<Vec<u8>>> {
        match self.read() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Replaces the file in place, or creates it on the first run.
    pub fn write_json(&self, value: &impl serde::Serialize, exists: bool) -> io::Result<()> {
        let encoded = format!(
            "{}\n",
            serde_json::to_string_pretty(value).map_err(io::Error::other)?
        );
        self.directory
            .write(&self.name, encoded.as_bytes(), !exists, exists)
    }
}
