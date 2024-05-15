#![allow(unused_imports)]

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use anchor_lang::{Discriminator, Owner};
use borsh::BorshDeserialize;
use bytemuck::{
  cast_slice,
  from_bytes,
  Pod,
  PodCastError,
  try_cast_slice,
  Zeroable,
};
use log::{debug, error, info};
use pyth_sdk_solana::PythError;
use pyth_sdk_solana::state::{AccountType, GenericPriceAccount, MAGIC, SolanaPriceAccount, VERSION_2};
use reqwest::{Client, StatusCode};
use reqwest::header::{HeaderMap, HeaderName};
use solana_account_decoder::UiAccountEncoding;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::{bs58, pubkey};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use common::*;
use decoder::{Decoder, Drift, drift_cpi, program_helpers, ProgramDecoder};
use decoder::decoded_account::{DecodedEpochAccount, JsonEpochAccount};
use timescale_client::TimescaleClient;

use crate::drift_cpi::OracleSource;
use crate::Trader;

pub struct EpochClient {
  pub signer: Keypair,
  pub rpc: RpcClient,
  pub client: TimescaleClient,
  pub decoder: Arc<ProgramDecoder>,
}

impl EpochClient {
  pub async fn new(signer: Keypair, rpc_url: String, timescale_db: String) -> anyhow::Result<Self> {
    Ok(Self {
      signer,
      rpc: RpcClient::new(rpc_url),
      client: TimescaleClient::new_from_url(timescale_db).await?,
      decoder: Arc::new(ProgramDecoder::new()?),
    })
  }

  pub fn rpc(&self) -> &RpcClient {
    &self.rpc
  }

  pub fn read_keypair_from_env(env_key: &str) -> anyhow::Result<Keypair> {
    read_keypair_from_env(env_key)
  }

  /// UNIX timestamp (in seconds) of the given slot.
  pub async fn get_slot_timestamp(
    &self,
    slot: u64,
    mainnet_rpc: String,
  ) -> anyhow::Result<Option<i64>> {
    let rpc_client = RpcClient::new(mainnet_rpc);
    let block = match rpc_client.get_block(slot).await {
      Ok(block) => block,
      Err(_) => return Ok(None),
    };
    Ok(block.block_time)
  }

  pub async fn get_program_accounts<A: BorshDeserialize + Owner + Discriminator>(
    &self,
  ) -> anyhow::Result<Vec<A>> {
    let memcmp = Memcmp::new_base58_encoded(0, A::discriminator().to_vec().as_slice());
    let filters = vec![RpcFilterType::Memcmp(memcmp)];

    let account_config = RpcAccountInfoConfig {
      encoding: Some(UiAccountEncoding::Base64),
      commitment: Some(CommitmentConfig::confirmed()),
      ..Default::default()
    };

    let keyed_accounts = self
      .rpc
      .get_program_accounts_with_config(
        &A::owner(),
        RpcProgramAccountsConfig {
          filters: Some(filters),
          account_config,
          ..Default::default()
        },
      )
      .await?;
    let markets: Vec<A> = keyed_accounts
      .into_iter()
      .flat_map(|(_, a)| A::deserialize(&mut a.data.as_slice()))
      .collect();
    Ok(markets)
  }

  //
  //
  // Build requests and deserialize responses
  //
  //

  async fn parse_borsh_response<T: BorshDeserialize>(
    res: reqwest::Response,
  ) -> anyhow::Result<T> {
    match res.status() {
      StatusCode::OK => Ok(T::deserialize(&mut res.bytes().await?.as_ref())?),
      _ => {
        let error_msg = res.text().await?;
        error!("Failed to parse Epoch response: {:?}", error_msg);
        Err(anyhow::anyhow!(error_msg))
      }
    }
  }

  //
  //
  // Interact with Timescale
  //
  //

  pub async fn account_id(
    &self,
    id: u64,
  ) -> anyhow::Result<Option<EpochAccount>> {
    let res = self
      .client
      .account_id(&QueryAccountId { id })
      .await?;
    match res {
      Some(a) => Ok(Some(a.to_epoch()?)),
      None => Ok(None),
    }
  }

  pub async fn accounts(
    &self,
    query: QueryAccounts,
  ) -> anyhow::Result<Vec<EpochAccount>> {
    let res = self
      .client
      .accounts(&query)
      .await?
      .into_iter()
      .flat_map(|a| a.to_epoch())
      .collect();
    Ok(res)
  }

  pub async fn borsh_decoded_accounts(
    &self,
    query: QueryDecodedAccounts,
  ) -> anyhow::Result<Vec<DecodedEpochAccount>> {
    let ts_accts = self
      .client
      .decoded_accounts(&query)
      .await?;
    // TODO: par iter
    let decoded_accts: Vec<DecodedEpochAccount> = ts_accts
      .into_iter()
      .flat_map(|a| match a.to_epoch() {
        Err(e) => {
          error!("Error converting ArchiveAccount to EpochAccount: {:?}", e);
          Err(anyhow::anyhow!(e))?
        }
        Ok(account) => {
          let name = match self
            .decoder
            .discrim_to_name(&query.owner, &account.data[..8].try_into()?)
          {
            Some(discrim) => Result::<_, anyhow::Error>::Ok(discrim),
            None => Err(anyhow::anyhow!("Invalid discriminant"))?,
          }?;
          let decoded =
            match self
              .decoder
              .borsh_decode_account(&query.owner, &name, &account.data)
            {
              Ok(decoded) => decoded,
              Err(e) => {
                error!("Error decoding account: {:?}", e);
                Err(anyhow::anyhow!(e))?
              }
            };
          Result::<_, anyhow::Error>::Ok(DecodedEpochAccount {
            key: account.key,
            slot: account.slot,
            owner: account.owner,
            decoded,
          })
        }
      })
      .collect();
    Ok(decoded_accts)
  }

  pub async fn json_decoded_accounts(
    &self,
    query: QueryDecodedAccounts,
  ) -> anyhow::Result<Vec<JsonEpochAccount>> {
    let ts_accts = self
      .client
      .decoded_accounts(&query)
      .await?;
    // TODO: par iter by making EpochAccount try from reference. Data must be borrowed Cow (use BytesWrapper)
    let mut decoded_accts: Vec<JsonEpochAccount> = ts_accts
      .into_iter()
      .flat_map(|a| match a.to_epoch() {
        Err(e) => {
          error!("Error converting ArchiveAccount to EpochAccount: {:?}", e);
          Err(anyhow::anyhow!(e))?
        }
        Ok(account) => {
          let name = match self
            .decoder
            .discrim_to_name(&query.owner, &account.data[..8].try_into()?)
          {
            Some(discrim) => Result::<_, anyhow::Error>::Ok(discrim),
            None => Err(anyhow::anyhow!("Invalid discriminant"))?,
          }?;
          let decoded = match self.decoder.json_decode_account(
            &query.owner,
            &name,
            &mut account.data.as_slice(),
          ) {
            Ok(decoded) => decoded,
            Err(e) => {
              error!("Error decoding account: {:?}", e);
              Err(anyhow::anyhow!(e))?
            }
          };
          Result::<_, anyhow::Error>::Ok(JsonEpochAccount {
            key: account.key,
            slot: account.slot,
            owner: account.owner,
            decoded,
          })
        }
      })
      .collect();
    // sort so the highest slot is 0th index
    decoded_accts.sort_by_key(|a| a.slot);
    decoded_accts.reverse();
    Ok(decoded_accts)
  }

  pub async fn registered_types(
    &self,
    query: Option<&QueryRegisteredTypes>
  ) -> anyhow::Result<Vec<RegisteredType>> {
    let res = match query {
      None => self.decoder.registered_types(),
      Some(query) => {
        Ok(self
          .decoder
          .registered_types()?
          .into_iter()
          .filter_map(|t| {
            match (&query.program_name, &query.program, &query.discriminant) {
              (Some(program_name), Some(program), Some(discriminant)) => {
                if t.program_name.to_lowercase() == *program_name.to_lowercase()
                  && t.program == *program
                  && t.discriminant.to_lowercase() == *discriminant.to_lowercase()
                {
                  Some(t)
                } else {
                  None
                }
              }
              (Some(program_name), Some(program), None) => {
                if t.program_name.to_lowercase() == *program_name.to_lowercase()
                  && t.program == *program
                {
                  Some(t)
                } else {
                  None
                }
              }
              (Some(program_name), None, Some(discriminant)) => {
                if t.program_name.to_lowercase() == *program_name.to_lowercase()
                  && t.discriminant.to_lowercase() == *discriminant.to_lowercase()
                {
                  Some(t)
                } else {
                  None
                }
              }
              (Some(program_name), None, None) => {
                if t.program_name.to_lowercase() == *program_name.to_lowercase() {
                  Some(t)
                } else {
                  None
                }
              }
              (None, Some(program), Some(discriminant)) => {
                if t.program == *program
                  && t.discriminant.to_lowercase() == *discriminant.to_lowercase()
                {
                  Some(t)
                } else {
                  None
                }
              }
              (None, Some(program), None) => {
                if t.program == *program {
                  Some(t)
                } else {
                  None
                }
              }
              (None, None, Some(discriminant)) => {
                if t.discriminant.to_lowercase() == *discriminant.to_lowercase() {
                  Some(t)
                } else {
                  None
                }
              }
              (None, None, None) => Some(t),
            }
          })
          .collect())
      }
    }?;
    Ok(res)
  }

  pub async fn highest_slot(&self) -> anyhow::Result<Option<u64>> {
    match self.client.highest_slot().await? {
      Some(s) => Ok(Some(s as u64)),
      None => Ok(None),
    }
  }

  pub async fn lowest_slot(&self) -> anyhow::Result<Option<u64>> {
    match self.client.lowest_slot().await? {
      Some(s) => Ok(Some(s as u64)),
      None => Ok(None),
    }
  }
}


/// Historical data: https://docs.drift.trade/historical-data/historical-data-v2
#[tokio::test]
async fn drift_traders() -> anyhow::Result<()> {
  init_logger();
  dotenv::dotenv().ok();
  let rpc_url = "http://localhost:8899".to_string();
  let signer = EpochClient::read_keypair_from_env("WALLET")?;
  let timescale_db = std::env::var("TIMESCALE_DB")?;
  let client = Arc::new(EpochClient::new(signer, rpc_url, timescale_db).await?);

  let max = client.highest_slot().await?;
  info!("highest slot: {:?}", max);
  let min = client.lowest_slot().await?;
  info!("lowest slot: {:?}", min);

  let pre_fetch = Instant::now();
  let users = client
    .borsh_decoded_accounts(
      QueryDecodedAccounts {
        owner: drift_cpi::ID,
        discriminant: "User".to_string(),
        limit: Some(1_000),
        ..Default::default()
      },
    )
    .await?;
  info!(
      "Time to fetch {} user accounts: {}s",
      &users.len(),
      pre_fetch.elapsed().as_millis() as f64 / 1000.0
  );

  // filter out users with same key
  let unique_users: HashMap<String, DecodedEpochAccount> = users
    .into_iter()
    .map(|user| (user.key.clone(), user))
    .collect();
  let mut users: Vec<DecodedEpochAccount> = unique_users.into_values().collect();
  info!("Unique users: {}", users.len());

  // sort where highest settled_perp_pnl is first index
  users.sort_by(|a, b| {
    let a = if let Decoder::Drift(drift_cpi::AccountType::User(user)) = &a.decoded {
      user.settled_perp_pnl
    } else {
      0
    };
    let b = if let Decoder::Drift(drift_cpi::AccountType::User(user)) = &b.decoded {
      user.settled_perp_pnl
    } else {
      0
    };
    b.cmp(&a)
  });
  let users = users
    .into_iter()
    .take(10)
    .collect::<Vec<DecodedEpochAccount>>();

  let pre_past_states = Instant::now();

  let mut traders = vec![];
  for user in users {
    let user_states: Vec<DecodedEpochAccount> = client
      .borsh_decoded_accounts(
        QueryDecodedAccounts {
          key: Some(Pubkey::from_str(&user.key)?),
          owner: drift_cpi::ID,
          discriminant: "User".to_string(),
          limit: Some(1_000),
          ..Default::default()
        },
      )
      .await?;
    debug!(
          "Fetched {} states for user: {}",
          user_states.len(),
          shorten_address(&Pubkey::from_str(&user.key)?)
      );
    // filter out duplicates with the same settled_perp_pnl value
    let mut updates: HashMap<i64, Data> = HashMap::new();
    for state in user_states.into_iter() {
      if let Decoder::Drift(drift_cpi::AccountType::User(user)) = state.decoded {
        let existing_value = updates.get(&user.settled_perp_pnl);
        if existing_value.is_none() {
          updates.insert(
            user.settled_perp_pnl,
            Data {
              x: state.slot as i64,
              y: trunc!(
                    user.settled_perp_pnl as f64/ program_helpers::QUOTE_PRECISION as f64,
                    2
                ),
            },
          );
        }
      }
    }
    let mut data: Vec<Data> = updates.into_values().collect();
    // sort with highest slot first
    data.sort_by(|a, b| b.x.cmp(&a.x));

    traders.push(Trader {
      key: Pubkey::from_str(&user.key)?,
      data,
    });
  }

  info!("Unfiltered traders: {}", traders.len());
  traders.retain(|trader| trader.avg_trade() > 0.0 && trader.worst_trade() > -30.0);
  info!("Filtered traders: {}", traders.len());

  info!(
      "Time to fetch and sort traders: {}s",
      pre_past_states.elapsed().as_millis() as f64 / 1000.0
  );

  let mut trader_keys = vec![];
  for trader in traders {
    info!(
        "{}, avg: {}%, worst: {}%, total: {}%",
        trader.key,
        trader.avg_trade(),
        trader.worst_trade(),
        trader.total_pct_pnl()
    );

    Plot::plot(
      vec![trader.data],
      &format!("{}/{}.png", env!("CARGO_MANIFEST_DIR"), trader.key),
      &format!("{} Trade History", trader.key),
      "PnL",
    )?;
    trader_keys.push(trader.key);
  }
  // write trader keys to file
  let trader_keys = serde_json::to_string(&trader_keys)?;
  let path = &format!("{}/trader_keys.json", env!("CARGO_MANIFEST_DIR"));
  std::fs::write(path, trader_keys)?;

  Ok(())
}
// 
// /// name: SOL-PERP index: 0
// /// name: BTC-PERP index: 1
// /// name: ETH-PERP index: 2
// /// name: APT-PERP index: 3
// /// name: 1MBONK-PERP index: 4
// /// name: MATIC-PERP index: 5
// /// name: ARB-PERP index: 6
// /// name: DOGE-PERP index: 7
// /// name: BNB-PERP index: 8
// /// name: SUI-PERP index: 9
// /// name: 1MPEPE-PERP index: 10
// /// name: OP-PERP index: 11
// ///
// /// NodeJS websocket: https://github.com/drift-labs/protocol-v2/blob/ebe773e4594bccc44e815b4e45ed3b6860ac2c4d/sdk/src/accounts/webSocketAccountSubscriber.ts#L174
// /// Rust websocket: https://github.com/drift-labs/drift-rs/blob/main/src/websocket_program_account_subscriber.rs
// /// Rust oracle type: https://github.com/drift-labs/protocol-v2/blob/ebe773e4594bccc44e815b4e45ed3b6860ac2c4d/programs/drift/src/state/oracle.rs#L126
// /// Pyth deser: https://github.com/pyth-network/pyth-sdk-rs/blob/main/pyth-sdk-solana/examples/get_accounts.rs#L67
// #[tokio::test]
// async fn drift_perp_markets() -> anyhow::Result<()> {
//   init_logger();
//   dotenv::dotenv().ok();
//   // let rpc_url = "http://localhost:8899".to_string();
//   let rpc_url = "https://guillemette-ldmq0k-fast-mainnet.helius-rpc.com/".to_string();
//   let signer = EpochClient::read_keypair_from_env("WALLET")?;
//   let timescale_db = std::env::var("TIMESCALE_DB")?;
//   let client = EpochClient::new(signer, rpc_url, timescale_db).await?;
// 
//   // let perp_markets = Drift::perp_markets(client.rpc()).await?;
//   // let spot_markets = Drift::spot_markets(client.rpc()).await?;
//   // let mut oracles: HashMap<String, (Pubkey, OracleSource)> = HashMap::new();
//   // for market in perp_markets {
//   //   let perp_name = Drift::decode_name(&market.name);
//   //   println!("name: {} index: {}", perp_name, market.market_index);
//   //   let spot_market = spot_markets
//   //     .iter()
//   //     .find(|spot| spot.market_index == market.quote_spot_market_index)
//   //     .ok_or(anyhow::anyhow!("Spot market not found"))?;
//   //   let oracle = spot_market.oracle;
//   //   let oracle_source = spot_market.oracle_source;
//   //   oracles.insert(perp_name, (oracle, oracle_source));
//   // }
//   // // sol_oracle: Gnt27xtC473ZT2Mw5u8wZ68Z3gULkSTb5DuxJy7eJotD
//   // // sol_oracle_source: PythStableCoin
//   // let (sol_oracle, sol_oracle_source) = oracles.get("SOL-PERP").ok_or(anyhow::anyhow!("SOL oracle not found"))?;
//   // println!("SOL oracle: {}", sol_oracle);
//   // println!("SOL oracle source: {:?}", sol_oracle_source);
// 
//   // let raw = client.rpc().get_account(sol_oracle).await?;
// 
//   let sol_oracle = pubkey!("Gnt27xtC473ZT2Mw5u8wZ68Z3gULkSTb5DuxJy7eJotD");
//   let sol_oracle_source = OracleSource::PythStableCoin;
//   let raw = client.rpc().get_account(&sol_oracle).await?;
//   let price_acct: &SolanaPriceAccount = load_price_account(&raw.data)?;
//   println!("price acct: {:#?}", price_acct);
//   let price_data = get_oracle_price(
//     
//   )
// 
//   Ok(())
// }
// 
// fn load<T: Pod>(data: &[u8]) -> Result<&T, PodCastError> {
//   let size = std::mem::size_of::<T>();
//   if data.len() >= size {
//     Ok(from_bytes(cast_slice::<u8, u8>(try_cast_slice(
//       &data[0..size],
//     )?)))
//   } else {
//     Err(PodCastError::SizeMismatch)
//   }
// }
// 
// /// Get a `Price` account from the raw byte value of a Solana account.
// pub fn load_price_account<const N: usize, T: Default + Copy + 'static>(
//   data: &[u8],
// ) -> Result<&GenericPriceAccount<N, T>, PythError> {
//   let pyth_price =
//     load::<GenericPriceAccount<N, T>>(data).map_err(|_| PythError::InvalidAccountData)?;
// 
//   if pyth_price.magic != MAGIC {
//     return Err(PythError::InvalidAccountData);
//   }
//   if pyth_price.ver != VERSION_2 {
//     return Err(PythError::BadVersionNumber);
//   }
//   if pyth_price.atype != AccountType::Price as u32 {
//     return Err(PythError::WrongAccountType);
//   }
// 
//   Ok(pyth_price)
// }