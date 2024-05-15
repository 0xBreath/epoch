use crate::{
    archive::{AppendVecIterator, ArchiveIterator},
    extract_snapshot::{ArchiveSnapshotExtractor, UnpackedSnapshotExtractor},
};
// use itertools::Itertools;
use log::info;
use reqwest::blocking::Response;
use std::{fs::File, path::Path};

/// Snapshot (archive) load options:
/// - file, a compressed tarball with extension .tar.zst
/// - unpacked, the uncompressed snapshot .tar.zst file
/// - streamed from HTTP endpoint
pub enum ArchiveLoader {
    Unpacked(UnpackedSnapshotExtractor),
    ArchiveFile(ArchiveSnapshotExtractor<File>),
    ArchiveDownload(ArchiveSnapshotExtractor<Response>),
}

/// Load a snapshot from a file or HTTP stream
impl ArchiveLoader {
    pub fn new(source: String) -> anyhow::Result<Self> {
        if source.starts_with("http://") || source.starts_with("https://") {
            ArchiveLoader::new_download(source)
        } else {
            ArchiveLoader::new_file(source.as_ref()).map_err(Into::into)
        }
    }

    fn new_download(url: String) -> anyhow::Result<ArchiveLoader> {
        let resp = reqwest::blocking::get(url)?;
        // compute number of Gb in resp
        let len = resp.content_length().unwrap_or(0);
        let len_gb = len / 1024 / 1024 / 1024;
        info!("Stream snapshot from HTTP ({} Gb)", len_gb);
        let loader = ArchiveSnapshotExtractor::from_reader(resp)?;
        Ok(ArchiveLoader::ArchiveDownload(loader))
    }

    fn new_file(path: &Path) -> anyhow::Result<ArchiveLoader> {
        Ok(if path.is_dir() {
            ArchiveLoader::Unpacked(UnpackedSnapshotExtractor::open(path)?)
        } else {
            ArchiveLoader::ArchiveFile(ArchiveSnapshotExtractor::open(path)?)
        })
    }
}

impl ArchiveIterator for ArchiveLoader {
    fn iter(&mut self) -> AppendVecIterator<'_> {
        match self {
            ArchiveLoader::Unpacked(loader) => Box::new(loader.iter()),
            ArchiveLoader::ArchiveFile(loader) => Box::new(loader.iter()),
            ArchiveLoader::ArchiveDownload(loader) => Box::new(loader.iter()),
        }
    }
}
