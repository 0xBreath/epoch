use std::sync::Arc;

use crossbeam_channel::Sender;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};

pub use archive::*;
pub use archiver::*;
use common::{ArchiveAccount, ChannelEvent};
pub use decode_accounts::*;
pub use errors::*;
pub use extract_snapshot::*;
pub use loader::*;

pub mod archive;
pub mod archiver;
pub mod decode_accounts;
pub mod errors;
pub mod extract_snapshot;
pub mod loader;

pub fn stream_archived_accounts(
  source: String,
  sender: Arc<Sender<ChannelEvent<ArchiveAccount>>>,
) -> anyhow::Result<()> {
  let mut loader = ArchiveLoader::new(source)?;
  let iter: Vec<anyhow::Result<AppendVecMeta>> = loader.iter().collect();
  iter.into_par_iter().try_for_each(|meta| {
    Archiver::extract_accounts(Arc::new(meta?), sender.clone())?;
    Result::<_, anyhow::Error>::Ok(())
  })
}
