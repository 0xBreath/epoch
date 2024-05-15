use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Clone, Serialize, Deserialize, BorshDeserialize, BorshSerialize)]
pub struct VaultBalance {
    pub amount: u64,
    pub ui_amount: f64,
    pub withheld_amount: u64,
    pub ui_withheld_amount: f64,
    pub decimals: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct EpochUser {
    pub profile: Pubkey,
    pub api_key: String,
    pub vault: Pubkey,
    pub balance: VaultBalance,
}
