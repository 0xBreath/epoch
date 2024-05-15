use std::str::FromStr;

use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use tokio_postgres::*;

use common::{ArchiveAccount, EpochAccount};

impl Eq for TimescaleAccount {}

/// Shadows [`ArchiveAccount`] but with postgres compatible types.
/// u64 -> i64
/// Pubkey -> String
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TimescaleAccount {
  pub id: i64,
  /// account key
  pub key: String,
  /// historical snapshot slot at which this state existed
  pub slot: i64,
  /// lamports in the account
  pub lamports: i64,
  /// the program that owns this account. If executable, the program that loads this account.
  pub owner: String,
  /// this account's data contains a loaded program (and is now read-only)
  pub executable: bool,
  /// the epoch at which this account will next owe rent
  pub rent_epoch: i64,
  /// data held in this account
  pub data: String
}

impl TimescaleAccount {
  pub fn new(account: ArchiveAccount) -> Self {
    let data = general_purpose::STANDARD.encode(&account.data);
    Self {
      id: account.id() as i64,
      key: account.key.to_string(),
      slot: account.slot as i64,
      lamports: account.lamports as i64,
      owner: account.owner.to_string(),
      executable: account.executable,
      rent_epoch: account.rent_epoch as i64,
      data,
    }
  }

  pub fn to_epoch(&self) -> anyhow::Result<EpochAccount> {
    Ok(EpochAccount {
      id: self.id as u64,
      key: self.key.clone(),
      slot: self.slot as u64,
      lamports: self.lamports as u64,
      owner: self.owner.clone(),
      executable: self.executable,
      rent_epoch: self.rent_epoch as u64,
      discriminant: None,
      data: general_purpose::STANDARD.decode(&self.data)?,
    })
  }
}

impl TryFrom<&Row> for TimescaleAccount {
  type Error = anyhow::Error;
  fn try_from(row: &Row) -> anyhow::Result<Self> {
    Ok(Self {
      id: row.get("id"),
      key: row.get("key"),
      slot: row.get("slot"),
      lamports: row.get("lamports"),
      owner: row.get("owner"),
      executable: row.get("executable"),
      rent_epoch: row.get("rent_epoch"),
      data: row.get("data"),
    })
  }
}

impl TryFrom<ArchiveAccount> for TimescaleAccount {
  type Error = anyhow::Error;
  fn try_from(account: ArchiveAccount) -> anyhow::Result<Self> {
    Ok(TimescaleAccount::new(account))
  }
}

pub trait TimescaleAccountTrait {
  fn to_archive(&self) -> anyhow::Result<ArchiveAccount>;
}

impl TimescaleAccountTrait for TimescaleAccount {
  fn to_archive(&self) -> anyhow::Result<ArchiveAccount> {
    Ok(ArchiveAccount {
      key: Pubkey::from_str(&self.key)?,
      slot: self.slot as u64,
      lamports: self.lamports as u64,
      owner: Pubkey::from_str(&self.owner)?,
      executable: self.executable,
      rent_epoch: self.rent_epoch as u64,
      data: general_purpose::STANDARD.decode(&self.data)?,
    })
  }
}
