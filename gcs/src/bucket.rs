use crate::download::download_file;
use chrono::{DateTime, NaiveDate, Utc};
use futures_util::future::join_all;
use log::*;
use regex::{Captures, Regex};
use reqwest::Url;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::{collections::HashMap, fs::File, io::Read, path::Path, str::FromStr, time::Instant};
use tokio::spawn;

pub enum GcsObjectsSource {
    Url(String),
    Path(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectResponse {
    pub kind: String,
    pub next_page_token: Option<String>,
    pub items: Option<Vec<GCSObject>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GCSObject {
    pub name: String,
    #[serde(deserialize_with = "deserialize_number_or_string")]
    pub size: u64,
    /// Media download link.
    pub media_link: String,
    /// The creation time of the object in RFC 3339 format.
    pub time_created: DateTime<Utc>,
}

fn deserialize_number_or_string<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Value = Deserialize::deserialize(deserializer)?;
    match value {
        Value::Number(n) => {
            if let Some(num) = n.as_u64() {
                Ok(num)
            } else {
                Err(serde::de::Error::custom("Invalid number format"))
            }
        }
        Value::String(s) => s.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("Expected number or string")),
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotMeta {
    pub snapshot: Snapshot,
    pub epoch: u64,
    pub bounds: Bounds,
}
impl SnapshotMeta {
    pub fn datetime(&self) -> String {
        self.snapshot
            .time_created
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }
    pub fn timestamp(&self) -> i64 {
        self.snapshot.time_created.timestamp()
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub name: String,
    pub filename: String,
    pub url: String,
    pub size: u64,
    pub slot: u64,
    pub hash: String,
    // #[serde(serialize_with = "Snapshot::serialize_date")]
    pub time_created: DateTime<Utc>,
}
// impl Snapshot {
//     fn serialize_date<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//      where
//        S: Serializer,
//     {
//         // convert to yyyy-mm-dd-hh-mm-ss
//         let res = self.time_created.format("%Y-%m-%d %H:%M:%S").to_string();
//         serializer.serialize_str(&res)
//     }
// }

#[derive(Clone, Debug, Default, Serialize)]
pub struct Bounds {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub start_slot: u64,
    pub end_slot: u64,
}

#[derive(Clone, Debug, Default)]
pub struct LedgerSnapshot {
    pub name: String,
    pub epoch: u64,
    pub url: String,
    pub size: u64,
    pub bounds: Bounds,
    pub version: String,
}

/// Default bucket to use: mainnet-beta-ledger-us-ny5
pub async fn list_objects(
    bucket_name: &str,
    next_page_token: &str,
    glob: Option<&str>,
) -> Result<ObjectResponse, reqwest::Error> {
    let base_url = format!(
        "https://storage.googleapis.com/storage/v1/b/{}/o/",
        bucket_name,
    );

    let mut url = Url::parse(&base_url).expect("Failed to parse base URL");

    let mut query_pairs = Vec::new();

    if !next_page_token.is_empty() {
        query_pairs.push(("pageToken", next_page_token));
    }

    if let Some(glob) = glob {
        query_pairs.push(("matchGlob", glob));
    }

    if !query_pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(query_pairs.into_iter());
    }

    debug!("requesting: {:?}", url);

    let response = reqwest::get(url.as_str()).await?;
    response.json().await
}

/// Pass Some("**/snapshot-*") to glob to only get snapshots (accounts db)
/// Pass None to get all objects, including epoch and bounds txt files, and hourly snapshots
pub async fn get_all_objects(
    bucket_name: &str,
    glob: Option<&str>,
) -> Result<Vec<ObjectResponse>, reqwest::Error> {
    let mut next_page_token = "".to_string();
    let mut all_objects = vec![];

    loop {
        let objects = list_objects(bucket_name, &next_page_token, glob).await?;

        let done = objects.next_page_token.is_none();
        next_page_token = objects.next_page_token.clone().unwrap_or_default();

        all_objects.push(objects);

        if done {
            break;
        }
    }
    Ok(all_objects)
}

pub fn search_objects_by_filename<'a>(
    objects: &'a [GCSObject],
    re: &'a Regex,
) -> Vec<(&'a GCSObject, regex::Captures<'a>)> {
    objects
        .iter()
        .filter_map(|object| re.captures(&object.name).map(|caps| (object, caps)))
        .collect()
}

async fn get_epoch_bounds(bounds_files: &[(&GCSObject, Captures<'_>)]) -> Vec<(u64, Bounds)> {
    let bounds_text_regex = Regex::new(r"Ledger has data for \d+ slots (\d+) to (\d+)").unwrap();

    let bounds_tasks = bounds_files.iter().map(|(s, captures)| {
        let bounds_text_regex = bounds_text_regex.clone();
        let epoch: u64 = captures.get(1).unwrap().as_str().parse().unwrap();

        let url = s.media_link.clone();
        let name = s.name.to_string();
        let size = s.size;

        spawn(async move {
            let downloaded_contents = download_file(Url::from_str(&url).unwrap()).await.ok()?;
            let downloaded_string = String::from_utf8(downloaded_contents).ok()?;

            let bounds_txt_captures = bounds_text_regex.captures(&downloaded_string)?;
            let start_slot: u64 = bounds_txt_captures.get(1)?.as_str().parse().ok()?;
            let end_slot: u64 = bounds_txt_captures.get(2)?.as_str().parse().ok()?;

            Some((
                epoch,
                Bounds {
                    name,
                    url,
                    size,
                    start_slot,
                    end_slot,
                },
            ))
        })
    });

    join_all(bounds_tasks)
        .await
        .into_iter()
        .filter_map(|r| r.ok()?)
        .collect()
}

async fn get_epochs_versions(versions_file: &[(&GCSObject, Captures<'_>)]) -> Vec<(u64, String)> {
    let bounds_tasks = versions_file.iter().map(|(s, captures)| {
        let epoch: u64 = captures.get(1).unwrap().as_str().parse().unwrap();
        let url = s.media_link.clone();
        spawn(async move {
            let downloaded_contents = download_file(Url::from_str(&url).unwrap()).await.ok()?;
            let downloaded_string = String::from_utf8(downloaded_contents).ok()?;
            Some((epoch, downloaded_string))
        })
    });

    join_all(bounds_tasks)
        .await
        .into_iter()
        .filter_map(|r| r.ok()?)
        .collect()
}

/// Filters out hourly snapshots
pub fn get_snapshots(objects: &[GCSObject]) -> Vec<Snapshot> {
    let snapshot_regex = Regex::new(r"snapshot-(\d+)-(\w+)\.tar\.(zst|bz2)").unwrap();
    search_objects_by_filename(objects, &snapshot_regex)
        .into_iter()
        .filter_map(|(s, captures)| {
            if s.name.contains("hourly") {
                None
            } else {
                let slot = captures.get(1).unwrap().as_str().parse().unwrap();
                let hash = captures.get(2).unwrap().as_str().to_string();
                let filename = s.name.split('/').last().unwrap().to_string();

                Some(Snapshot {
                    name: s.name.clone(),
                    filename,
                    url: s.media_link.clone(),
                    size: s.size,
                    slot,
                    hash,
                    time_created: s.time_created,
                })
            }
        })
        .collect()
}

pub async fn get_ledger_snapshots(objects: &[GCSObject]) -> Vec<LedgerSnapshot> {
    let rocksdb_file_regex = Regex::new(r"(\d+)/rocksdb\.tar\.bz2").unwrap();
    let bounds_file_regex = Regex::new(r"(\d+)/bounds\.txt").unwrap();
    let version_file_regex = Regex::new(r"(\d+)/version\.txt").unwrap();

    // need rocksdb for everything
    let mut snapshots: HashMap<u64, LedgerSnapshot> =
        search_objects_by_filename(objects, &rocksdb_file_regex)
            .into_iter()
            .map(|(s, captures)| {
                let epoch: u64 = captures.get(1).unwrap().as_str().parse().unwrap();
                (
                    epoch,
                    LedgerSnapshot {
                        name: s.name.clone(),
                        epoch,
                        url: s.media_link.clone(),
                        size: s.size,
                        bounds: Bounds::default(),
                        version: String::default(),
                    },
                )
            })
            .collect();

    // grab the bounds.txt files, download the contents, and attempt to parse them
    let epochs_bounds =
        get_epoch_bounds(&search_objects_by_filename(objects, &bounds_file_regex)).await;
    for (epoch, bounds) in epochs_bounds {
        if let Some(ledger) = snapshots.get_mut(&epoch) {
            ledger.bounds = bounds;
        }
    }

    let epochs_version =
        get_epochs_versions(&search_objects_by_filename(objects, &version_file_regex)).await;
    for (epoch, version) in epochs_version {
        if let Some(ledger) = snapshots.get_mut(&epoch) {
            ledger.version = version;
        }
    }

    snapshots.drain().map(|(_, s)| s).collect()
}

/// 1. Get all snapshots in google cloud storage bucket
/// 2. Get all epoch bounds.txt files, which defined start and end slots for each epoch
/// 3. For each snapshot, find the epoch and slot bounds it belongs to
/// 4. Sort snapshots by slot
pub async fn __remote_snapshot_metas(bucket: &str) -> anyhow::Result<Vec<ObjectResponse>> {
    info!("Fetching snapshots from GCS, this usually takes 60-90s");
    let pre = Instant::now();
    let resp = get_all_objects(bucket, None).await?;
    info!("Fetch all GCS objects in: {}s", pre.elapsed().as_secs());
    // write response to json file
    let json = serde_json::to_string_pretty(&resp)?;
    std::fs::write("gcs_snapshots.json", json)?;
    Ok(resp)
}

/// For development, load GCS snapshots from local file path. This avoids the 90 seconds GCS response time.
pub async fn __local_snapshot_metas(path: &str) -> anyhow::Result<Vec<ObjectResponse>> {
    info!("Fetching snapshots from local file path");
    let path = Path::new(path);
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let resp = serde_json::from_slice::<Vec<ObjectResponse>>(&buf)?;
    Ok(resp)
}

/// Hourly snapshots on the same day are filtered to the snapshot closest to 17:00:00 UTC
fn filter_hourly_snapshots(snapshots: Vec<Snapshot>) -> Vec<Snapshot> {
    let target_hour = 17;
    let mut grouped: HashMap<NaiveDate, Vec<Snapshot>> = HashMap::new();

    // Group dates by year, month, and day
    snapshots.into_iter().for_each(|snapshot| {
        grouped
            .entry(snapshot.time_created.date_naive())
            .or_default()
            .push(snapshot);
    });

    let mut filtered_dates: Vec<Snapshot> = Vec::new();

    for (_, mut group) in grouped {
        // Sort by absolute difference from target hour (5 PM)
        group.sort_by_key(|k| {
            let time_created = k.time_created.naive_utc();
            let target_time = k
                .time_created
                .date_naive()
                .and_hms_opt(target_hour, 0, 0)
                .unwrap();
            if time_created > target_time {
                // convert NaiveDateTime to DateTime<Utc>
                time_created.signed_duration_since(target_time)
            } else {
                target_time.signed_duration_since(time_created)
            }
        });
        // Take the first element after sorting, which is the closest to 5 PM
        if let Some(closest) = group.first() {
            filtered_dates.push(closest.clone());
        }
    }

    filtered_dates
}

pub async fn get_snapshot_metas(src: GcsObjectsSource) -> anyhow::Result<Vec<SnapshotMeta>> {
    let resp = match src {
        GcsObjectsSource::Url(url) => __remote_snapshot_metas(&url).await,
        GcsObjectsSource::Path(path) => __local_snapshot_metas(&path).await,
    }?;

    let objects: Vec<GCSObject> = resp.into_iter().filter_map(|o| o.items).flatten().collect();

    let snapshots_hourly = get_snapshots(&objects);
    // filter out snapshots on the say day, and take the closest to 5pm UTC
    let snapshots = filter_hourly_snapshots(snapshots_hourly);

    let mut metas_map = HashMap::<u64, SnapshotMeta>::new();

    let bounds_file_regex = Regex::new(r"(\d+)/bounds\.txt").unwrap();
    let epochs_bounds =
        get_epoch_bounds(&search_objects_by_filename(&objects, &bounds_file_regex)).await;

    for (epoch, bounds) in epochs_bounds {
        snapshots.iter().for_each(|s| {
            if s.slot >= bounds.start_slot && s.slot <= bounds.end_slot {
                metas_map.insert(
                    s.slot,
                    SnapshotMeta {
                        snapshot: s.clone(),
                        epoch,
                        bounds: bounds.clone(),
                    },
                );
            }
        });
    }

    let mut metas: Vec<SnapshotMeta> = metas_map.into_values().collect();
    metas.sort_by_key(|s| s.snapshot.slot);
    Ok(metas)
}
