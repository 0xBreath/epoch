use bb8::Pool;
use bb8_postgres::PostgresConnectionManager;
use log::*;
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use postgres_types::ToSql;
use tokio_postgres::{Client, Config, Statement};

use common::{QueryAccountId, QueryAccounts, QueryDecodedAccounts};
use decoder::ProgramDecoder;

use crate::account::TimescaleAccount;
use crate::settings::DatabaseSettings;

struct QueryParams {
  pub query: String,
  pub args: Vec<Box<dyn ToSql + Sync>>,
}

pub struct TimescaleClient {
  client: Client,
  upsert_acct_stmt: Statement,
}

impl TimescaleClient {
  pub async fn new_from_url(connection_url: String) -> anyhow::Result<Self> {
    let client = match DatabaseSettings::new_from_url(connection_url) {
      Err(err) => return Err(anyhow::anyhow!("Error loading configuration: {}", err)),
      Ok(config) => Self::new(&config).await?,
    };
    Ok(client)
  }

  pub async fn new(config: &DatabaseSettings) -> anyhow::Result<Self> {
    let pool = Self::connect_to_db(config).await?;
    let client = pool.dedicated_connection().await?;

    let upsert_acct_stmt = client.prepare("
        INSERT INTO accounts (id, key, slot, lamports, owner, executable, rent_epoch, data)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (id, slot)
        DO NOTHING;
    ").await?;

    Ok(Self {
      client,
      upsert_acct_stmt,
    })
  }

  async fn connect_to_db(
    config: &DatabaseSettings,
  ) -> anyhow::Result<Pool<PostgresConnectionManager<MakeTlsConnector>>> {
    let db_url = if let Some(connection_string) = &config.connection_string {
      connection_string.clone()
    } else {
      if config.host.is_none() || config.username.is_none() {
        let error = anyhow::anyhow!("Missing host or username in database configuration");
        return Err(error);
      }
      if config.database_name.is_none() {
        format!(
          "host={} user={} password={} port={}",
          config.host.as_ref().unwrap(),
          config.username.as_ref().unwrap(),
          config.password.as_ref().unwrap(),
          config.port.unwrap_or(5432)
        )
      } else {
        format!(
          "host={} user={} password={} port={} dbname={}",
          config.host.as_ref().unwrap(),
          config.username.as_ref().unwrap(),
          config.password.as_ref().unwrap(),
          config.port.unwrap_or(5432),
          config.database_name.as_ref().unwrap(),
        )
      }
    };
    let config = db_url.parse::<Config>()?;

    let connector = TlsConnector::builder()
      .danger_accept_invalid_certs(true)
      .build()?;
    let connector = MakeTlsConnector::new(connector);

    let manager = PostgresConnectionManager::new(
      config,
      connector,
    );
    let pool = Pool::builder().build(manager).await?;

    Ok(pool)
  }

  // =================================== QUERIES ===================================

  /// The row ID is a hash of the account data and key, so if the account state is the same slot to slot,
  /// then the upsert will overwrite the existing row with the same data but whatever slot is in the updated data.
  /// Slots don't really matter if the data is the same across time.
  pub async fn upsert_account(&self, account: &TimescaleAccount) -> anyhow::Result<u64> {
    let stmt = &self.upsert_acct_stmt;
    Ok(self.client.execute(
      stmt,
      &[
        &account.id,
        &account.key.clone(),
        &account.slot,
        &account.lamports,
        &account.owner,
        &account.executable,
        &account.rent_epoch,
        &account.data,
      ],
    ).await?)
  }

  pub async fn upsert_accounts(&mut self, accounts: Vec<TimescaleAccount>) -> anyhow::Result<()> {
    let stmt = &self.upsert_acct_stmt;
    let transaction = self.client.transaction().await?;
    for account in accounts {
      transaction.execute(
        stmt,
        &[
          &account.id,
          &account.key.clone(),
          &account.slot,
          &account.lamports,
          &account.owner,
          &account.executable,
          &account.rent_epoch,
          &account.data
        ],
      ).await?;
    }
    Ok(transaction.commit().await?)
  }

  pub async fn account_id(
    &self,
    query: &QueryAccountId,
  ) -> anyhow::Result<Option<TimescaleAccount>> {
    let stmt = "SELECT * FROM accounts WHERE id = $1";
    let stmt = self.client.prepare(stmt).await?;

    let result = self.client.query(&stmt, &[&(query.id as i64)]).await?;
    match result.first() {
      None => Ok(None),
      Some(acct) => Ok(Some(TimescaleAccount::try_from(acct)?)),
    }
  }

  fn build_accounts_query(&self, params: &QueryAccounts) -> anyhow::Result<QueryParams> {
    let mut query = "SELECT * FROM accounts".to_string();
    let mut where_added = false;
    let mut index = 1;
    let mut args: Vec<Box<dyn ToSql + Sync>> = vec![];

    if let Some(key) = &params.key {
      query = format!("{} WHERE key = ${}", &query, index);
      where_added = true;
      index += 1;
      args.push(Box::new(key.to_string()));
    }

    let clause = match where_added {
      false => "WHERE",
      true => "AND",
    };
    match &params.slot {
      Some(slot) => {
        query = format!("{} {} slot = ${}", &query, clause, index);
        where_added = true;
        index += 1;
        args.push(Box::new(*slot as i64));
      }
      None => match (params.min_slot, params.max_slot) {
        (Some(min_slot), Some(max_slot)) => {
          if max_slot - min_slot > 10 {
            return Err(anyhow::anyhow!("Slot range too large for free demo"));
          } else {
            query = format!(
              "{} {} slot >= ${} AND slot <= ${}",
              &query,
              clause,
              index,
              index + 1
            );
            where_added = true;
            index += 2;
            args.push(Box::new(min_slot as i64));
            args.push(Box::new(max_slot as i64));
          }
        }
        (Some(_min_slot), None) => {
          return Err(anyhow::anyhow!(
              "Missing max slot for query (free demo restriction)"
          ));
        }
        (None, Some(_max_slot)) => {
          return Err(anyhow::anyhow!(
              "Missing min slot for query (free demo restriction)"
          ));
        }
        _ => {}
      },
    };

    if let Some(owner) = &params.owner {
      let clause = match where_added {
        false => "WHERE",
        true => "AND",
      };
      query = format!("{} {} owner = ${}", &query, clause, index);
      // where_added = true;
      // index += 1;
      args.push(Box::new(owner.to_string()));
    }

    if let Some(limit) = &params.limit {
      if *limit > 100 {
        return Err(anyhow::anyhow!("Query limit too large for free demo"));
      } else {
        query = format!("{} LIMIT {}", &query, limit);
      }
    } else {
      query = format!("{} LIMIT 10", &query);
    }
    if let Some(offset) = &params.offset {
      query = format!("{} OFFSET {}", &query, offset);
    }

    debug!("accounts query: {:#?}", query);
    Ok(QueryParams { query, args })
  }

  pub async fn accounts(&self, query: &QueryAccounts) -> anyhow::Result<Vec<TimescaleAccount>> {
    let builder = self.build_accounts_query(query)?;
    let stmt = self.client.prepare(&builder.query).await?;
    let args: Vec<&(dyn ToSql + Sync)> = builder.args.iter().map(|obj| obj.as_ref()).collect();

    let accts = self.client.query(&stmt, &args).await?.into_iter().flat_map(|row| TimescaleAccount::try_from(&row)).collect::<Vec<TimescaleAccount>>();
    Ok(accts)
  }

  fn build_decoded_accounts_query(
    &self,
    params: &QueryDecodedAccounts,
  ) -> anyhow::Result<QueryParams> {
    let mut query = "SELECT * FROM accounts".to_string();
    let mut where_added = false;
    let mut index = 1;
    let mut args: Vec<Box<dyn ToSql + Sync>> = vec![];

    if let Some(key) = &params.key {
      query = format!("{} WHERE key = ${}", &query, index);
      where_added = true;
      index += 1;
      args.push(Box::new(key.to_string()));
    }

    {
      let clause = match where_added {
        false => "WHERE",
        true => "AND",
      };
      match params.slot {
        Some(slot) => {
          query = format!("{} {} slot = ${}", &query, clause, index);
          where_added = true;
          index += 1;
          args.push(Box::new(slot as i64));
        }
        None => match (params.min_slot, params.max_slot) {
          (Some(min_slot), Some(max_slot)) => {
            if max_slot - min_slot > 10 {
              return Err(anyhow::anyhow!("Slot range too large for free demo"));
            } else {
              query = format!(
                "{} {} slot >= ${} AND slot <= ${}",
                &query,
                clause,
                index,
                index + 1
              );
              where_added = true;
              index += 2;
              args.push(Box::new(min_slot as i64));
              args.push(Box::new(max_slot as i64));
            }
          }
          (Some(_min_slot), None) => {
            return Err(anyhow::anyhow!(
                "Missing max slot for query (free demo restriction)"
            ));
          }
          (None, Some(_max_slot)) => {
            return Err(anyhow::anyhow!(
                "Missing min slot for query (free demo restriction)"
            ));
          }
          _ => {}
        },
      };
    }

    {
      let clause = match where_added {
        false => "WHERE",
        true => "AND",
      };
      query = format!("{} {} owner = ${}", &query, clause, index);
      index += 1;
      args.push(Box::new(params.owner.to_string()));

      let base64_discrim = ProgramDecoder::name_to_base64_discrim(&params.discriminant);
      query = format!(
        "{} AND TO_BASE64(SUBSTR(FROM_BASE64(data), 1, 8)) = ${}",
        &query, index
      );
      args.push(Box::new(base64_discrim));
      // index += 1;
      // where_added = true;
    }

    if let Some(limit) = &params.limit {
      if *limit > 100 {
        return Err(anyhow::anyhow!("Query limit too large for free demo"));
      } else {
        query = format!("{} LIMIT {}", &query, limit);
      }
    } else {
      query = format!("{} LIMIT 10", &query);
    }
    if let Some(offset) = &params.offset {
      query = format!("{} OFFSET {}", &query, offset);
    }

    debug!("decoded accounts query: {:#?}", query);
    Ok(QueryParams { query, args })
  }

  pub async fn decoded_accounts(
    &self,
    query: &QueryDecodedAccounts,
  ) -> anyhow::Result<Vec<TimescaleAccount>> {
    let builder = self.build_decoded_accounts_query(query)?;
    let stmt = self.client.prepare(&builder.query).await?;
    let args: Vec<&(dyn ToSql + Sync)> = builder.args.iter().map(|obj| obj.as_ref()).collect();

    let accts = self.client.query(&stmt, &args).await?.into_iter().flat_map(|row| TimescaleAccount::try_from(&row)).collect::<Vec<TimescaleAccount>>();
    Ok(accts)
  }

  pub async fn delete_accounts_by_key(
    &self,
    query: &QueryAccounts,
  ) -> anyhow::Result<u64> {
    match query.key {
      None => Err(anyhow::anyhow!("Missing key to delete Timescale accounts")),
      Some(key) => {
        let stmt = "DELETE FROM accounts WHERE key = $1";
        let stmt = self.client.prepare(stmt).await?;

        let accts = self.client.execute(&stmt, &[&key.to_string()]).await?;
        Ok(accts)
      }
    }
  }

  pub async fn highest_slot(&self) -> anyhow::Result<Option<i64>> {
    let stmt = "SELECT MAX(slot) FROM accounts";
    let stmt = self.client.prepare(stmt).await?;
    let result = self.client.query(&stmt, &[]).await?;
    match result.first() {
      None => Ok(None),
      Some(row) => Ok(row.get(0)),
    }
  }

  pub async fn lowest_slot(&self) -> anyhow::Result<Option<i64>> {
    let stmt = "SELECT MINT(slot) FROM accounts";
    let stmt = self.client.prepare(stmt).await?;
    let result = self.client.query(&stmt, &[]).await?;
    match result.first() {
      None => Ok(None),
      Some(row) => Ok(row.get(0)),
    }
  }
}

#[tokio::test]
async fn crud_account() -> anyhow::Result<()> {
  use common::init_logger;
  use solana_sdk::pubkey::Pubkey;

  init_logger();
  dotenv::dotenv().ok();
  let db_url = std::env::var("TIMESCALE_DB")?;
  let client = TimescaleClient::new_from_url(db_url).await?;
  let key = Pubkey::new_unique();
  let owner = Pubkey::new_unique();

  // delete everything to start with a fresh slate
  let deleted = client.delete_accounts_by_key(&QueryAccounts {
    key: Some(key),
    ..Default::default()
  }).await?;
  info!("Reset {} rows", deleted);

  // CREATE
  let acct = TimescaleAccount {
    id: 0,
    key: key.to_string(),
    slot: 1,
    lamports: 0,
    owner: owner.to_string(),
    executable: false,
    rent_epoch: 0,
    data: "".to_string(),
  };
  let modified_rows = client.upsert_account(&acct).await?;
  info!("should have created 1 row, and modified 0 because it's a new row");
  assert_eq!(modified_rows, 0);

  // READ
  let read_rows = client.accounts(&QueryAccounts {
    key: Some(key),
    slot: Some(1),
    owner: Some(owner),
    ..Default::default()
  }).await?;
  info!("should have read 1 row");
  assert_eq!(read_rows.len(), 1);

  // UPDATE
  let updated_acct = TimescaleAccount {
    id: 0,
    key: key.to_string(),
    slot: 1,
    lamports: 0,
    owner: owner.to_string(),
    executable: false,
    rent_epoch: 0,
    data: "".to_string(),
  };
  // should not update any rows since the ID and slot are the same as first version
  let modified_rows = client.upsert_account(&updated_acct).await?;
  info!("should have updated 0 rows because id-slot index is the same");
  assert_eq!(modified_rows, 0);

  // CREATE second account
  let new_owner = Pubkey::find_program_address(&[&key.to_bytes()], &owner).0;
  let acct = TimescaleAccount {
    id: 0,
    key: key.to_string(),
    slot: 1,
    lamports: 0,
    owner: new_owner.to_string(),
    executable: false,
    rent_epoch: 0,
    data: "".to_string(),
  };
  let modified_rows = client.upsert_account(&acct).await?;
  info!("should have created 0 rows, and modified 0 because it's the same id-slot index");
  assert_eq!(modified_rows, 0);

  // READ two accounts
  let read_rows = client.accounts(&QueryAccounts {
    key: Some(key),
    slot: Some(1),
    owner: Some(owner),
    ..Default::default()
  }).await?;
  info!("should have read 1 row");
  assert_eq!(read_rows.len(), 1);

  // DELETE
  let deleted = client.delete_accounts_by_key(&QueryAccounts {
    key: Some(key),
    ..Default::default()
  }).await?;
  info!("should have deleted 1 row");
  assert_eq!(deleted, 1);

  // check everything was deleted
  let leftover = client.accounts(&QueryAccounts {
    key: Some(key),
    slot: Some(1),
    owner: Some(owner),
    ..Default::default()
  }).await?;
  info!("should be 0 rows remaining after deletion");
  assert!(leftover.is_empty());

  Ok(())
}
