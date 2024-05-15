# Epoch

Backfill will request, download, and decode every snapshot from GCS provided in the `backfill.yaml` config file.
It uses `gcs` to fetch and parse GCS snapshot metadata, such as the slot, epoch, and name.
It uses `stream_archived_accounts` from the `snapshot` crate to handle the decoding of each snapshot into its 
accounts.