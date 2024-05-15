# Archive Stream

This library does the following

* Unpacks a snapshot `.tar.zst` from a HTTP endpoint or file path

* Decodes the snapshot to get the `AppendVec` which is the "snapshot" of accounts at that point in time

* Deserializes each account from a byte offset within `AppendVec`, and returns as `ArchiveAccount` which is a 
  combination of the Solana account struct and the slot at which it existed.

* Streams each `ArchiveAccount` to the callback provided to `stream_archived_accounts`