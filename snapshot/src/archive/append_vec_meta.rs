use crate::decode_accounts::{AppendVec, StoredAccountMeta};
use common::ArchiveAccount;
use solana_sdk::clock::Slot;
use std::{ffi::OsStr, str::FromStr, sync::Arc};

pub struct AppendVecMeta {
    pub append_vec: AppendVec,
    pub slot: Slot,
}

pub struct StoredAccountMetaHandle {
    meta: Arc<AppendVecMeta>,
    offset: usize,
}

impl StoredAccountMetaHandle {
    pub fn new(meta: Arc<AppendVecMeta>, offset: usize) -> StoredAccountMetaHandle {
        Self { meta, offset }
    }

    pub fn access(&self) -> Option<StoredAccountMeta<'_>> {
        let res = self.meta.append_vec.get_account(self.offset)?;
        Some(res.0)
    }

    pub fn snapshot_account(&self) -> Option<ArchiveAccount> {
        let account = self.access()?;
        Some(ArchiveAccount {
            key: account.meta.pubkey,
            slot: self.meta.slot,
            lamports: account.account_meta.lamports,
            owner: account.account_meta.owner,
            executable: account.account_meta.executable,
            rent_epoch: account.account_meta.rent_epoch,
            data: account.data.to_vec(),
        })
    }
}

pub fn parse_append_vec_name(name: &OsStr) -> Option<(u64, u64)> {
    let name = name.to_str()?;
    let mut parts = name.splitn(2, '.');
    let slot = u64::from_str(parts.next().unwrap_or(""));
    let id = u64::from_str(parts.next().unwrap_or(""));
    match (slot, id) {
        (Ok(slot), Ok(version)) => Some((slot, version)),
        _ => None,
    }
}
