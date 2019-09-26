#![feature(async_await)]

use failure::prelude::*;
use chain_service::chain_service::ChainService;
use chain_client::{ChainClient, watch_stream::WatchStream};
use crypto::HashValue;
use tokio::runtime::TaskExecutor;
use logger::prelude::*;
use types::{event::EventKey, account_address::AccountAddress, access_path::AccessPath, transaction::{SignedTransaction, Version}};
use futures::{
    sync::mpsc::UnboundedReceiver,
    Stream, Poll,
};
use star_types::{proto::{chain::{WatchData}}, channel_transaction::ChannelTransactionRequest};
use atomic_refcell::{AtomicRefCell};
use std::sync::Arc;
use core::borrow::{BorrowMut};
use std::sync::mpsc;
use types::proof::SparseMerkleProof;
use types::transaction::SignedTransactionWithProof;

pub mod mock_star_client;
pub mod mock_star_node;

#[derive(Clone)]
pub struct MockChainClient {
    //exe: TaskExecutor,
    chain_service: Arc<AtomicRefCell<ChainService>>,
}

impl MockChainClient {
    pub fn new(exe: TaskExecutor) -> (Self, mpsc::Receiver<()>) {
        let (chain_service, receiver) = ChainService::new(&exe, &None);
        let client = Self {
            //exe,
            chain_service: Arc::new(AtomicRefCell::new(chain_service)),
        };
        (client, receiver)
    }

}

pub struct MockStreamReceiver<T> {
    inner_rx: UnboundedReceiver<T>
}

impl<T> Stream for MockStreamReceiver<T> {
    type Item = T;
    type Error = grpcio::Error;

    fn poll(&mut self) -> Poll<Option<T>, Self::Error> {
        self.inner_rx.poll().map_err(|e| { grpcio::Error::RemoteStopped })
    }
}

impl ChainClient for MockChainClient {
    fn get_account_state_with_proof(&self, address: &AccountAddress, version: Option<Version>) -> Result<(Version, Option<Vec<u8>>, SparseMerkleProof)> {
        let chain_service = self.chain_service.as_ref().borrow();
        let (version, state,proof) = chain_service.get_account_state_with_proof_inner(address, version).ok_or(format_err!("Can not find account state by address: {}", address))?;
        Ok((version,state.map(|state|state.as_ref().to_vec()),proof))
    }

    fn faucet(&self, address: AccountAddress, amount: u64) -> Result<()> {
        let chain_service = self.chain_service.as_ref().borrow();
        chain_service.faucet_inner(address, amount).map(|_| ())
    }

    fn submit_transaction(&self, signed_transaction: SignedTransaction) -> Result<()> {
        let chain_service = self.chain_service.as_ref().borrow();
        chain_service.send_tx(signed_transaction);

        Ok(())
    }

    fn watch_transaction(&self, address: &AccountAddress, seq_num: u64) -> Result<Option<SignedTransactionWithProof>> {
//        let chain_service = self.chain_service.as_ref().borrow();
//        let rx = chain_service.watch_transaction_inner(*address, ver);
//        let stream = MockStreamReceiver { inner_rx: rx };
//        Ok(WatchStream::new(stream))
        unimplemented!()
    }

    fn get_transaction_by_seq_num(&self, address: &AccountAddress, seq_num: u64) -> Result<Option<SignedTransactionWithProof>> {
        let chain_service = self.chain_service.as_ref().borrow();
        chain_service.get_transaction_by_seq_num_inner(address.clone(), seq_num)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;
    use crate::mock_star_client::MockStarClient;

    #[test]
    fn test_mock_client() {
        let rt = Runtime::new().unwrap();
        let (client, handle) = MockStarClient::new();
        let state = client.get_account_state_with_proof(&AccountAddress::default(), None).unwrap().1.unwrap();
        println!("state: {:#?}", state)
    }

    #[test]
    fn test_mock_faucet() {
        let rt = Runtime::new().unwrap();
        let (client, handle) = MockStarClient::new();
        let mut state = client.get_account_state_with_proof(&AccountAddress::default(), None).unwrap().1.unwrap();
        println!("state: {:#?}", state);
        let receiver = AccountAddress::random();
        client.faucet(receiver, 100);
        client.faucet(receiver, 100);
        state = client.get_account_state_with_proof(&AccountAddress::default(), None).unwrap().1.unwrap();
        println!("state: {:#?}", state);
    }

    #[test]
    fn test_faucet_state() {
        let mut rt1 = Runtime::new().unwrap();
        let (mock_chain_service, handle) = MockStarClient::new();
        let client = Arc::new(mock_chain_service);
        let mut state = client.get_account_state_with_proof(&AccountAddress::default(), None).unwrap().1.unwrap();
        println!("state: {:#?}", state);
        let receiver = AccountAddress::random();
        client.faucet(receiver, 100);
        client.faucet(receiver, 100);
        state = client.get_account_state_with_proof(&AccountAddress::default(), None).unwrap().1.unwrap();
        println!("state: {:#?}", state);
    }
}
