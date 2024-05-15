use std::sync::Arc;

use crossbeam_channel::Sender;
use rayon::{iter::ParallelIterator, prelude::IntoParallelRefIterator};

use common::archive_account::ArchiveAccount;
use common::ChannelEvent;

use crate::{AppendVecMeta, SnapshotError, StoredAccountMetaHandle};

/// Archiver handles everything related to extracting accounts from a snapshot
/// 1. Load snapshot from file or HTTP source
/// 2. Decode AppendVec
/// 3. Iterate raw accounts in AppendVec
/// 4. Deserialize archived accounts in AppendVec
/// 5. Send each ArchiveAccount to channel
pub struct Archiver;

impl Archiver {
  pub fn extract_accounts(
    meta: Arc<AppendVecMeta>,
    sender: Arc<Sender<ChannelEvent<ArchiveAccount>>>,
  ) -> anyhow::Result<()> {
    let mut offsets = Vec::<usize>::new();
    let mut offset = 0_usize;
    loop {
      match meta.append_vec.get_account(offset) {
        None => break,
        Some((_, next_offset)) => {
          offsets.push(offset);
          offset = next_offset;
        }
      }
    }

    offsets
      .par_iter()
      .try_for_each(|offset| {
        let handle = StoredAccountMetaHandle::new(Arc::clone(&meta), *offset);
        let account = match handle.snapshot_account() {
          Some(account) => Ok(account),
          None => Err(anyhow::Error::from(SnapshotError::AccountAccessFailed)),
        }?;
        let msg = ChannelEvent::Msg(account);
        sender.send(msg)?;
        Result::<_, anyhow::Error>::Ok(())
      })
  }
}
