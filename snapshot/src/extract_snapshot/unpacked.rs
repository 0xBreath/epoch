use crate::{
    archive::{parse_append_vec_name, AppendVecIterator, AppendVecMeta, ArchiveIterator},
    decode_accounts::{
        deserialize_from, AccountsDbFields, AppendVec, DeserializableVersionedBank,
        SerializableAccountStorageEntry,
    },
    SnapshotError,
};
use itertools::Itertools;
use log::info;
use solana_runtime::snapshot_utils::SNAPSHOT_STATUS_CACHE_FILENAME;
use std::{
    fs::OpenOptions,
    io::BufReader,
    path::{Path, PathBuf},
    str::FromStr,
    time::Instant,
};

pub const SNAPSHOTS_DIR: &str = "snapshots";

/// Extracts account data from snapshots that were unarchived to a file system.
pub struct UnpackedSnapshotExtractor {
    root: PathBuf,
    accounts_db_fields: AccountsDbFields<SerializableAccountStorageEntry>,
}

impl ArchiveIterator for UnpackedSnapshotExtractor {
    fn iter(&mut self) -> AppendVecIterator<'_> {
        Box::new(self.unboxed_iter())
    }
}

impl UnpackedSnapshotExtractor {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let snapshots_dir = path.join(SNAPSHOTS_DIR);
        let status_cache = snapshots_dir.join(SNAPSHOT_STATUS_CACHE_FILENAME);
        if !status_cache.is_file() {
            return Err(anyhow::Error::from(SnapshotError::NoStatusCache));
        }

        let snapshot_files = snapshots_dir.read_dir()?;

        let snapshot_file_path = snapshot_files
            .filter_map(|entry| entry.ok())
            .find(|entry| u64::from_str(&entry.file_name().to_string_lossy()).is_ok())
            .map(|entry| entry.path().join(entry.file_name()))
            .ok_or(SnapshotError::NoSnapshotManifest)?;

        info!("Opening snapshot manifest: {:?}", snapshot_file_path);
        let snapshot_file = OpenOptions::new().read(true).open(&snapshot_file_path)?;
        let mut snapshot_file = BufReader::new(snapshot_file);

        let pre_unpack = Instant::now();
        let versioned_bank: DeserializableVersionedBank = deserialize_from(&mut snapshot_file)?;
        drop(versioned_bank);
        let versioned_bank_post_time = Instant::now();

        let accounts_db_fields: AccountsDbFields<SerializableAccountStorageEntry> =
            deserialize_from(&mut snapshot_file)?;
        let accounts_db_fields_post_time = Instant::now();
        drop(snapshot_file);

        info!(
            "Read bank fields in {:?}",
            versioned_bank_post_time - pre_unpack
        );
        info!(
            "Read accounts DB fields in {:?}",
            accounts_db_fields_post_time - versioned_bank_post_time
        );

        Ok(UnpackedSnapshotExtractor {
            root: path.to_path_buf(),
            accounts_db_fields,
        })
    }

    pub fn unboxed_iter(&self) -> impl Iterator<Item = anyhow::Result<AppendVecMeta>> + '_ {
        std::iter::once(self.iter_streams())
            .flatten_ok()
            .flatten_ok()
    }

    fn iter_streams(
        &self,
    ) -> anyhow::Result<impl Iterator<Item = anyhow::Result<AppendVecMeta>> + '_> {
        let accounts_dir = self.root.join("accounts");
        Ok(accounts_dir
            .read_dir()?
            .filter_map(|f| f.ok())
            .filter_map(|f| {
                let name = f.file_name();
                parse_append_vec_name(&f.file_name()).map(move |parsed| (parsed, name))
            })
            .map(move |((slot, version), name)| {
                self.open_append_vec(slot, version, &accounts_dir.join(name))
            }))
    }

    fn open_append_vec(&self, slot: u64, id: u64, path: &Path) -> anyhow::Result<AppendVecMeta> {
        let known_vecs = self
            .accounts_db_fields
            .0
            .get(&slot)
            .map(|v| &v[..])
            .unwrap_or(&[]);
        let known_vec = known_vecs.iter().find(|entry| entry.id == (id as usize));
        let known_vec = match known_vec {
            None => return Err(anyhow::Error::from(SnapshotError::UnexpectedAppendVec)),
            Some(v) => v,
        };

        let append_vec = AppendVec::new_from_file(path, known_vec.accounts_current_len)?;
        Ok(AppendVecMeta { append_vec, slot })
    }
}
