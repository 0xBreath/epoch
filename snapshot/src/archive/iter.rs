use std::sync::Arc;

use rayon::{iter::ParallelIterator, prelude::IntoParallelRefIterator};

use crate::archive::{AppendVecMeta, StoredAccountMetaHandle};

pub type AppendVecIterator<'a> = Box<dyn Iterator<Item=anyhow::Result<AppendVecMeta>> + 'a>;

pub trait ArchiveIterator: Sized {
  fn iter(&mut self) -> AppendVecIterator<'_>;
}

pub fn append_vec_iter(meta: Arc<AppendVecMeta>) -> Vec<StoredAccountMetaHandle> {
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
  let res: Vec<StoredAccountMetaHandle> = offsets
    .par_iter()
    .map(move |offset| StoredAccountMetaHandle::new(Arc::clone(&meta), *offset))
    .collect();
  res
}
