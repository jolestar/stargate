use std::sync::Arc;

use rand::prelude::*;

use chain_client::{ChainClient, RpcChainClient};
use mock_chain_client::MockChainClient;
use crypto::test_utils::KeyPair;
use crypto::Uniform;
use types::account_address::AccountAddress;

use super::wallet::*;
use types::account_config::coin_struct_tag;
use logger::prelude::*;
use std::thread::sleep;
use failure::_core::time::Duration;
use tokio::runtime::{Runtime,TaskExecutor};
use failure::prelude::*;
use crypto::ed25519::{Ed25519PrivateKey, Ed25519PublicKey};

#[test]
fn test_wallet() -> Result<()> {
    ::logger::init_for_e2e_testing();
    let sender_amount: u64 = 10_000_000;
    let receiver_amount: u64 = 10_000_000;
    let sender_fund_amount: u64 = 5_000_000;
    let receiver_fund_amount: u64 = 4_000_000;
    let transfer_amount = 1_000_000;

    let mut rng0: StdRng = SeedableRng::from_seed([0; 32]);
    let mut rng1: StdRng = SeedableRng::from_seed([1; 32]);

    let sender_keypair:KeyPair<Ed25519PrivateKey, Ed25519PublicKey> = KeyPair::generate_for_testing(&mut rng0);
    let receiver_keypair:KeyPair<Ed25519PrivateKey, Ed25519PublicKey> = KeyPair::generate_for_testing(&mut rng1);

    let mut rt = Runtime::new()?;
    let executor = rt.executor();

    let client = Arc::new(MockChainClient::new(executor.clone()));
    let sender = AccountAddress::from_public_key(&sender_keypair.public_key);
    let receiver = AccountAddress::from_public_key(&receiver_keypair.public_key);

    debug!("sender_address: {}", sender);
    debug!("receiver_address: {}", receiver);
    client.faucet(sender, sender_amount)?;
    client.faucet(receiver, receiver_amount)?;

    let wallet = Wallet::new_with_client(executor, sender, sender_keypair, client)?;
    assert_eq!(sender_amount, wallet.balance());
    let asset_tag = coin_struct_tag();
    let fund_txn = wallet.fund(asset_tag.clone(), receiver,sender_fund_amount, receiver_fund_amount)?;
    //debug!("txn:{:#?}", fund_txn);
    wallet.apply_txn(&fund_txn)?;
    let sender_channel_balance = wallet.channel_balance(asset_tag.clone(),receiver)?;
    assert_eq!(sender_channel_balance, sender_fund_amount);

    let transfer_txn = wallet.transfer(asset_tag.clone(), receiver, transfer_amount)?;
    //debug!("txn:{:#?}", transfer_txn);
    wallet.apply_txn(&transfer_txn)?;

    let sender_channel_balance = wallet.channel_balance(asset_tag.clone(),receiver)?;
    assert_eq!(sender_channel_balance, sender_fund_amount - transfer_amount);

    let withdraw_txn = wallet.withdraw(asset_tag.clone(), receiver, sender_channel_balance, 1)?;
    debug!("txn:{:#?}", withdraw_txn);
    wallet.apply_txn(&withdraw_txn)?;

    let sender_channel_balance = wallet.channel_balance(asset_tag.clone(),receiver)?;
    assert_eq!(sender_channel_balance, 0);

    Ok(())
}
