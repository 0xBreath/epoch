use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use clap::Parser;
use log::*;

use common::{ArchiveAccount, ChannelEvent, init_logger, RingBuffer};
use config::*;
use gcs::bucket::*;
use snapshot::stream_archived_accounts;
use timescale_client::{TimescaleAccount, TimescaleClient};

// use rayon::prelude::{IntoParallelIterator, ParallelIterator};

mod config;
mod errors;

#[derive(Parser, Debug)]
struct Args {
  /// Path to backfill.yaml config.
  /// Should deserialize into BackfillConfig
  #[arg(long, env, default_value = "backfill.yaml")]
  config_file_path: PathBuf,
}

fn main() -> anyhow::Result<()> {
  dotenv::dotenv().ok();
  init_logger();
  let args: Args = Args::parse();
  info!("🏗️ Backfill snapshots with args: {:?}", args);

  let config = BackfillConfig::read_config(&args.config_file_path)?;

  let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

  let mut timescale: Option<TimescaleClient> = match config.timescale_db {
    None => None,
    Some(timescale_db) => {
      Some(rt.block_on(async move {
        TimescaleClient::new_from_url(timescale_db).await
      })?)
    }
  };

  let bucket = config.gcs_bucket;
  let metas: Vec<SnapshotMeta> = rt.block_on(async move {
    let metas = match &config.gcs_local_file {
      Some(path) => match Path::new(path).exists() {
        false => get_snapshot_metas(GcsObjectsSource::Url(bucket)).await,
        true => get_snapshot_metas(GcsObjectsSource::Path(path.clone())).await,
      },
      None => get_snapshot_metas(GcsObjectsSource::Url(bucket)).await,
    }?;
    Result::<_, anyhow::Error>::Ok(metas)
  })?;
  info!(
      "GCS snapshots found: {} - {}",
      metas.first().unwrap().snapshot.slot,
      metas.last().unwrap().snapshot.slot
  );

  // slice SnapshotMetas to range config wants to backfill
  let start = config.start_date;
  let end = config.end_date;
  let metas: Vec<_> = metas.into_iter().filter(|m| m.snapshot.time_created >= start && m.snapshot.time_created <= end).collect();
  info!(
      "Snapshot date range to backfill: {} - {}",
      metas.first().unwrap().datetime(),
      metas.last().unwrap().datetime()
  );

  let (tx, rx) = crossbeam_channel::unbounded::<ChannelEvent<ArchiveAccount>>();
  let programs = Arc::new(config.programs);
  rt.spawn(async move {
    let programs = programs.clone();

    let mut total_read = 0;
    const BUFFER_SIZE: usize = 100;
    let mut buffer = RingBuffer::new(BUFFER_SIZE);

    // upsert accounts to Timescale
    while let Ok(msg) = rx.recv() {
      match msg {
        ChannelEvent::Msg(account) => {
          if programs.contains(&account.owner) {
            if let Some(timescale) = &mut timescale {
              let timescale_acct = TimescaleAccount::new(account);
              if buffer.full() {
                let take = buffer.take();
                assert!(buffer.is_empty());
                if let Err(e) = timescale.upsert_accounts(take).await {
                  error!("Failed to upsert accounts: {:#?}", e);
                } else {
                  total_read += BUFFER_SIZE;
                  info!("Total accounts processed: {}", total_read)
                }
              }
              buffer.push(timescale_acct);
            }
          }
        }
        ChannelEvent::Done => {
          info!("Snapshot done");
          if let Some(timescale) = &mut timescale {
            if let Err(e) = timescale.upsert_accounts(buffer.take()).await {
              error!("Failed to upsert accounts: {:#?}", e);
            }
            info!("Total accounts processed: {}", total_read);
          }
        }
      }
    }
  });

  // backfill from the most recent date to the oldest
  let sender = Arc::new(tx);
  for meta in metas {
    let source = meta.snapshot.url.clone();
    info!("Backfilling snapshot: {:#?}", &meta.datetime());
    match stream_archived_accounts(source, sender.clone()) {
      Ok(_) => {
        info!(
            "Done snapshot {} for slot {}",
            &meta.datetime(),
            &meta.snapshot.slot
        );
      }
      Err(e) => {
        error!("Error backfilling snapshot: {}", e);
      }
    }
  }
  Ok(())
}
