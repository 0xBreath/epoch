use std::{path::PathBuf, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Deserialize)]
pub struct BackfillConfig {
  /// Number of threads to ETL (extract, transform, and load) snapshots.
  pub max_workers: usize,
  /// Only parse and store accounts for these programs.
  #[serde(deserialize_with = "BackfillConfig::deserialize_pubkeys")]
  pub programs: Vec<Pubkey>,
  /// Earliest date, formatted yyyy-mm-dd, to start backfilling from.
  #[serde(deserialize_with = "BackfillConfig::deserialize_date")]
  pub start_date: DateTime<Utc>,
  /// Latest date, formatted yyyy-mm-dd, to start backfilling from.
  #[serde(deserialize_with = "BackfillConfig::deserialize_date")]
  pub end_date: DateTime<Utc>,
  /// Google cloud storage bucket to pull snapshots from
  pub gcs_bucket: String,
  /// Optional local file path to gcs [`ObjectResponse`]. Primarily for development to speed up iteration.
  pub gcs_local_file: Option<String>,
  /// GCS service account JSON file
  pub gcs_sa_key: String,
  /// Timescale db connection url
  pub timescale_db: String
}

impl BackfillConfig {
  fn deserialize_pubkeys<'de, D>(deserializer: D) -> Result<Vec<Pubkey>, D::Error>
                                 where
                                   D: Deserializer<'de>,
  {
    let keys: Vec<String> = Vec::deserialize(deserializer)?;
    keys.into_iter()
        .map(|pubkey| Pubkey::from_str(&pubkey).map_err(serde::de::Error::custom))
        .collect()
  }

  fn deserialize_date<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
                              where
                                D: Deserializer<'de>,
  {
    // deserialize yyyy-mm-dd into DateTime<Utc>
    let date: String = match String::deserialize(deserializer) {
      Ok(date) => date,
      Err(e) => {
        return Err(serde::de::Error::custom(format!(
          "Failed to deserialize date: {}",
          e
        )))
      }
    };
    let yyyy = &date.split('-').collect::<Vec<&str>>()[0];
    let mm = &date.split('-').collect::<Vec<&str>>()[1];
    let dd = &date.split('-').collect::<Vec<&str>>()[2];
    let dt_fixed =
      match DateTime::parse_from_rfc3339(&format!("{}-{}-{}T00:00:00Z", yyyy, mm, dd)) {
        Ok(dr) => dr,
        Err(e) => {
          return Err(serde::de::Error::custom(format!(
            "Failed to parse date: {}",
            e
          )))
        }
      };
    let dt = DateTime::<Utc>::from(dt_fixed);
    Ok(dt)
  }

  pub fn read_config(path: &PathBuf) -> anyhow::Result<BackfillConfig> {
    let contents = String::from_utf8(std::fs::read(path)?)?;
    Ok(serde_yaml::from_str(&contents)?)
  }
}
