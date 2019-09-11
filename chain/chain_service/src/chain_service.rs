use failure::prelude::*;
use types::proto::{access_path::AccessPath as ProtoAccessPath, account_state_blob::{AccountStateBlob as AccountStateBlobProto, AccountStateWithProof}};
use types::{proof::SparseMerkleProof, account_state_blob::AccountStateBlob, account_config::association_address, transaction::{SignedTransaction, TransactionPayload, RawTransaction}, access_path::AccessPath, account_address::AccountAddress};
use futures::{sync::mpsc::{unbounded, UnboundedReceiver}, future::Future, sink::Sink, stream::Stream};
use super::pub_sub;
use grpcio::WriteFlags;
use state_storage::{StateStorage, AccountState};
use super::transaction_storage::TransactionStorage;
use std::{sync::{Arc, Mutex}, time::Duration, convert::TryFrom};
use crypto::{hash::CryptoHash, HashValue};
use grpc_helpers::provide_grpc_response;
use vm_genesis::{encode_genesis_transaction, encode_transfer_program, encode_create_account_program, GENESIS_KEYPAIR};
use metrics::IntGauge;
use futures03::{
    future::{FutureExt, TryFutureExt},
    stream::StreamExt,
    sink::SinkExt,
    executor::block_on,
};
use tokio::runtime::{TaskExecutor, Runtime};
use star_types::{channel_transaction::ChannelTransaction,
                 proto::{chain_grpc::Chain,
                         chain::{LeastRootRequest, LeastRootResponse,
                                 FaucetRequest, FaucetResponse,
                                 GetAccountStateWithProofRequest, GetAccountStateWithProofResponse, Blob,
                                 WatchTransactionRequest,
                                 MempoolAddTransactionStatus, MempoolAddTransactionStatusCode,
                                 SubmitTransactionRequest, SubmitTransactionResponse,
                                 StateByAccessPathResponse, AccountResource,
                                 WatchEventRequest,
                                 GetTransactionByHashRequest, GetTransactionByHashResponse,
                                 WatchData, WatchTxData,
                         },
                         channel_transaction::ChannelTransaction as ChannelTransactionProto,
                 }, transaction_output_helper};
use vm_runtime::{MoveVM, VMExecutor};
use lazy_static::lazy_static;
use config::config::VMConfig;
use state_view::StateView;
use core::borrow::{Borrow, BorrowMut};
use proto_conv::{FromProto, IntoProto};
use types::contract_event::ContractEvent;
use types::event::EventKey;
use types::transaction::{TransactionOutput, TransactionStatus};
use types::vm_error::{VMStatus};
use super::event_storage::EventStorage;
use atomic_refcell::AtomicRefCell;
use futures::sync::mpsc::UnboundedSender;
use futures::future::FutureResult;
use logger::prelude::*;

lazy_static! {
    static ref VM_CONFIG:VMConfig = VMConfig::onchain();
}

#[derive(Clone)]
pub struct ChainService {
    sender: channel::Sender<SignedTransaction>,
    state_db: Arc<Mutex<StateStorage>>,
    tx_db: Arc<Mutex<TransactionStorage>>,
    event_storage: Arc<AtomicRefCell<EventStorage>>,
    tx_pub: Arc<Mutex<pub_sub::Pub<WatchData>>>,
    event_pub: Arc<Mutex<pub_sub::Pub<WatchData>>>,
    task_exe: TaskExecutor,
}

impl ChainService {
    pub fn new(exe: &TaskExecutor) -> Self {
        let gauge = IntGauge::new("receive_transaction_channel_counter", "receive transaction channel").unwrap();
        let (tx_sender, mut tx_receiver) = channel::new(1_024, &gauge);
        let tx_db = Arc::new(Mutex::new(TransactionStorage::new()));
        let state_db = Arc::new(Mutex::new(StateStorage::new()));
        let event_storage = Arc::new(AtomicRefCell::new(EventStorage::new()));
        let tx_pub = Arc::new(Mutex::new(pub_sub::Pub::new()));
        let event_pub = Arc::new(Mutex::new(pub_sub::Pub::new()));
        let chain_service = ChainService { sender: tx_sender, state_db, tx_db, event_storage, tx_pub, event_pub, task_exe: exe.clone() };
        let chain_service_clone = chain_service.clone();

        let receiver_future = async move {
            while let Some(tx) = tx_receiver.next().await {
                chain_service_clone.apply_on_chain_transaction(tx);
            };
        };

        exe.spawn(receiver_future.boxed().unit_error().compat());

        let genesis_checked_txn = encode_genesis_transaction(&GENESIS_KEYPAIR.0, GENESIS_KEYPAIR.1.clone());
        let genesis_txn = genesis_checked_txn.into_inner();
        match genesis_txn.payload() {
            TransactionPayload::WriteSet(ws) => {
                let tmp_state_db = chain_service.state_db.lock().unwrap();
                let state_hash = tmp_state_db.apply_write_set(&ws).unwrap();
                let mut tmp_tx_db = chain_service.tx_db.lock().unwrap();
                tmp_tx_db.insert_tx(state_hash, HashValue::zero(), genesis_txn);
            }
            _ => {}
        };
        chain_service
    }

    fn apply_on_chain_transaction(&self, signed_tx: SignedTransaction) {
        let signed_tx_hash = signed_tx.hash();
        let mut tx_db = self.tx_db.lock().unwrap();
        let ver = tx_db.least_version();
        let exist_flag = tx_db.exist_signed_transaction(signed_tx_hash);
        let mut watch_tx = WatchTxData::new();
        if !exist_flag {
            let state_db = self.state_db.lock().unwrap();
            let mut output_vec = MoveVM::execute_block(vec![signed_tx.clone()], &VM_CONFIG, &*state_db);
            let output = output_vec.pop().expect("execute txn at least has one output");
            info!("apply_on_chain_transaction tx:{}, output: {}", signed_tx.raw_txn().hash(), output);
            match output.status() {
                TransactionStatus::Keep(_) => {
                    let state_hash = state_db.apply_libra_output(&output).unwrap();
                    let mut event_storage_mut = self.event_storage.as_ref().borrow_mut();
                    let event_hash = event_storage_mut.insert_events(ver + 1, output.events()).expect("insert event err.");
                    tx_db.insert_tx(state_hash, event_hash, signed_tx.clone());
                }
                _ => {
                }
            }
            let event_lock = self.event_pub.lock().unwrap();
            output.events().iter().for_each(|e| {
                let event = e.clone().into_proto();
                let mut event_resp = WatchData::new();
                event_resp.set_event(event);
                event_lock.send(event_resp).unwrap();
            });

            watch_tx.set_output(transaction_output_helper::into_pb(output).unwrap());


            let mut wt_resp = WatchData::new();
            watch_tx.set_signed_txn(signed_tx.into_proto());

            wt_resp.set_tx(watch_tx);

            let tx_lock = self.tx_pub.lock().unwrap();
            tx_lock.send(wt_resp).unwrap();
        }
    }

    pub fn watch_transaction_inner(&self, address: AccountAddress, index: u64) -> UnboundedReceiver<WatchData> {
        if index != std::u64::MAX {
            //TODO
            //1. get least tx index
            //2. compare index
            //3. get tx and send to client
        }

        let (sender, receiver) = unbounded::<WatchData>();
        //TODO id generate.
        let id = HashValue::random();
        let tx_lock = self.tx_pub.lock().unwrap();
        tx_lock.subscribe(id, sender, Box::new(move |tx: WatchData| -> bool {
            let signed_tx = SignedTransaction::from_proto(tx.get_tx().get_signed_txn().clone()).unwrap();
            signed_tx.sender() == address
        }));

        receiver
    }

    pub fn watch_event_inner(&self, address: AccountAddress, keys: Vec<EventKey>, _index: u64) -> UnboundedReceiver<WatchData> {
        let (sender, receiver) = unbounded::<WatchData>();
        let id = address.hash();
        let event_lock = self.event_pub.lock().unwrap();
        event_lock.subscribe(id, sender, Box::new(move |data: WatchData| -> bool {
            let mut flag = false;
            let event: ContractEvent = ContractEvent::from_proto(data.get_event().clone()).unwrap();
            keys.iter().for_each(|key| {
                flag = key == event.key()
            });

            flag
        }));

        receiver
    }

    pub fn least_state_root_inner(&self) -> HashValue {
        self.tx_db.lock().unwrap().least_hash_root()
    }

    pub fn get_account_state_inner(&self, account_address: &AccountAddress, ver: Option<u64>) -> Option<Vec<u8>> {
        let state_db = self.state_db.lock().unwrap();
        match ver {
            Some(version) => { state_db.get_account_state_by_version(version, account_address) }
            None => {
                state_db.get_account_state(account_address)
            }
        }
    }

    pub fn get_account_state_with_proof_inner(&self, account_address: &AccountAddress, ver: Option<u64>) -> Option<(u64, Option<AccountStateBlob>, SparseMerkleProof)> {
        let state_db = self.state_db.lock().unwrap();
        state_db.account_state_with_proof(ver, account_address)
    }

    pub fn state_by_access_path_inner(&self, account_address: AccountAddress, path: Vec<u8>) -> Result<Option<Vec<u8>>> {
        let state_db = self.state_db.lock().unwrap();
        state_db.get(&AccessPath::new(account_address, path))
    }

    pub fn faucet_inner(&self, receiver: AccountAddress, amount: u64) -> Result<()> {
        let state_db = self.state_db.lock().unwrap();
        let exist_flag = state_db.exist_account(&receiver);
        let program = if !exist_flag {
            encode_create_account_program(&receiver, amount)
        } else {
            encode_transfer_program(&receiver, amount)
        };

        let sender = association_address();//AccountAddress::from_public_key(&GENESIS_KEYPAIR.1);
        let s_n = match state_db.sequence_number(&sender) {
            Some(num) => num,
            _ => 0
        };
        drop(state_db);
        let signed_tx = RawTransaction::new(
            sender,
            s_n,
            program,
            1000_000 as u64,
            1 as u64,
            Duration::from_secs(u64::max_value()),
        ).sign(&GENESIS_KEYPAIR.0, GENESIS_KEYPAIR.1.clone())
            .unwrap()
            .into_inner();

        self.apply_on_chain_transaction(signed_tx);
        Ok(())
    }

    pub fn send_tx(&self, txn: SignedTransaction) {
        let mut sender_tmp = self.sender.clone();
        let send_future = async move {
            sender_tmp.send(txn).await.unwrap();
        };

        self.task_exe.clone().spawn(send_future.boxed().unit_error().compat());
    }

    pub fn get_transaction_by_hash(&self, hash: HashValue) -> Result<SignedTransaction> {
        let lock = self.tx_db.lock().unwrap();
        let signed_tx = lock.get_signed_transaction_by_hash(&hash);
        match signed_tx {
            Some(tx) => Ok(tx),
            None => bail!("could not find tx by hash {}",hash),
        }
    }
}

impl Chain for ChainService {
    fn least_state_root(&mut self, ctx: ::grpcio::RpcContext, _req: LeastRootRequest, sink: ::grpcio::UnarySink<LeastRootResponse>) {
        let least_hash_root = self.least_state_root_inner();
        let mut resp = LeastRootResponse::new();
        resp.set_state_root_hash(least_hash_root.to_vec());
        provide_grpc_response(Ok(resp), ctx, sink);
    }

    fn faucet(&mut self, ctx: ::grpcio::RpcContext,
              req: FaucetRequest,
              sink: ::grpcio::UnarySink<FaucetResponse>) {
        let resp = AccountAddress::try_from(req.get_address().to_vec()).and_then(|account_address| {
            self.faucet_inner(account_address, req.get_amount())
        }).and_then(|_| {
            Ok(FaucetResponse::new())
        });
        provide_grpc_response(resp, ctx, sink);
    }

    fn get_account_state_with_proof(&mut self, ctx: ::grpcio::RpcContext,
                                    req: GetAccountStateWithProofRequest,
                                    sink: ::grpcio::UnarySink<GetAccountStateWithProofResponse>) {
        let resp = AccountAddress::try_from(req.get_address().to_vec()).and_then(|account_address| {
            let ver = if req.has_ver() { Some(req.get_ver()) } else { None };
            Ok(self.get_account_state_with_proof_inner(&account_address, ver))
        }).and_then(|query| {
            let mut get_resp = GetAccountStateWithProofResponse::new();
            match query {
                Some((v, a, p)) => {
                    get_resp.set_version(v);
                    get_resp.set_sparse_merkle_proof(p.into_proto());

                    match a {
                        Some(account) => {
                            get_resp.set_account_state_blob(account.into_proto());
                        }
                        _ => {}
                    }
                }
                None => {}
            };
            Ok(get_resp)
        });
        provide_grpc_response(resp, ctx, sink);
    }

    fn submit_transaction(&mut self, ctx: ::grpcio::RpcContext,
                          req: SubmitTransactionRequest,
                          sink: ::grpcio::UnarySink<SubmitTransactionResponse>) {
        let resp = SignedTransaction::from_proto(req.signed_txn.clone().unwrap()).and_then(|signed_txn| {
            self.send_tx(signed_txn);

            let mut submit_resp = SubmitTransactionResponse::new();
            let mut state = MempoolAddTransactionStatus::new();
            state.set_code(MempoolAddTransactionStatusCode::Valid);
            submit_resp.set_mempool_status(state);
            Ok(submit_resp)
        });

        provide_grpc_response(resp, ctx, sink);
    }

    fn watch_transaction(&mut self, ctx: ::grpcio::RpcContext,
                         req: WatchTransactionRequest,
                         sink: ::grpcio::ServerStreamingSink<WatchData>) {
        let index = if req.has_index() {
            req.get_index()
        } else {
            std::u64::MAX
        };
        let receiver = self.watch_transaction_inner(AccountAddress::from_proto(req.address).unwrap(), index);
        let stream = receiver
            .map(|e| (e, WriteFlags::default()))
            .map_err(|_| grpcio::Error::RemoteStopped);

        ctx.spawn(
            sink
                .send_all(stream)
                .map(|_| println!("completed"))
                .map_err(|e| println!("failed to reply: {:?}", e)),
        );
    }

    fn state_by_access_path(&mut self, ctx: ::grpcio::RpcContext,
                            req: ProtoAccessPath,
                            sink: ::grpcio::UnarySink<StateByAccessPathResponse>) {
        let resp = AccountAddress::try_from(req.get_address().to_vec()).and_then(|account_address| {
            self.state_by_access_path_inner(account_address, req.path)
        }).and_then(|resource| {
            let mut state_resp = StateByAccessPathResponse::new();
            match resource {
                Some(re) => {
                    let mut a_r = AccountResource::new();
                    a_r.set_resource(re);
                    state_resp.set_account_resource(a_r);
                }
                _ => {}
            };
            Ok(state_resp)
        });

        provide_grpc_response(resp, ctx, sink);
    }

    fn watch_event(&mut self, ctx: ::grpcio::RpcContext,
                   req: WatchEventRequest,
                   sink: ::grpcio::ServerStreamingSink<WatchData>) {
        let index = if req.has_index() {
            req.get_index()
        } else {
            std::u64::MAX
        };
        let keys = req.get_keys().iter().map(|key| -> EventKey { EventKey::new(u8_32(key.get_key())) }).collect();
        let receiver = self.watch_event_inner(AccountAddress::try_from(req.get_address().to_vec()).unwrap(), keys, index);
        let stream = receiver
            .map(|e| (e, WriteFlags::default()))
            .map_err(|_| grpcio::Error::RemoteStopped);

        ctx.spawn(
            sink
                .send_all(stream)
                .map(|_| println!("completed"))
                .map_err(|e| println!("failed to reply: {:?}", e)),
        );
    }

    fn get_transaction_by_hash(&mut self, ctx: ::grpcio::RpcContext,
                               req: GetTransactionByHashRequest,
                               sink: ::grpcio::UnarySink<GetTransactionByHashResponse>) {
        let hash = HashValue::from_slice(req.get_state_root_hash()).unwrap();
        let mut resp = GetTransactionByHashResponse::new();
        let lock = self.tx_db.lock().unwrap();
        let signed_tx = lock.get_signed_transaction_by_hash(&hash);
        match signed_tx {
            Some(tx) => resp.set_signed_tx(tx.into_proto()),
            None => {}
        }
        provide_grpc_response(Ok(resp), ctx, sink);
    }
}

fn u8_32(value: &[u8]) -> [u8; 32] {
    let mut tmp = [0u8; 32];
    tmp.copy_from_slice(value);
    tmp
}

#[cfg(test)]
mod tests {
    use vm_genesis::{encode_genesis_transaction, GENESIS_KEYPAIR};
    use crate::chain_service::ChainService;
    use tokio::runtime::Runtime;
    use futures::future::Future;
    use futures03::{
        future::{FutureExt, TryFutureExt},
        stream::StreamExt,
        sink::SinkExt,
        executor::block_on,
    };
    use std::{thread, time};
    use compiler::Compiler;
    use types::{account_address::AccountAddress, transaction::{Program, RawTransaction}, account_config::{core_code_address, association_address}};
    use std::{time::Duration};
    use crypto::hash::{CryptoHash, TransactionInfoHasher};

    #[test]
    fn test_genesis() {
        let genesis_checked_txn = encode_genesis_transaction(&GENESIS_KEYPAIR.0, GENESIS_KEYPAIR.1.clone());
        let genesis_txn = genesis_checked_txn.into_inner();
        println!("{:?}", genesis_txn);
    }

    #[test]
    fn test_chain_service() {
        let mut rt = Runtime::new().unwrap();
        let exe = rt.executor();
        let chain_service = ChainService::new(&exe);
//        let print_future = async move {
//            let ten_millis = time::Duration::from_millis(100);
//            thread::sleep(ten_millis);
        let root = chain_service.least_state_root_inner();
        println!("{:?}", root);
//        };
//        rt.block_on(print_future.boxed().unit_error().compat()).unwrap();
    }

    #[test]
    fn test_apply_program() {
        let code =
            "
            main() {
                return;
            }
            ";

        let compiler = Compiler {
            code,
            ..Compiler::default()
        };

        let program = compiler.into_program(vec![]).unwrap();

        let account_address = association_address();

        let mut rt = Runtime::new().unwrap();
        let exe = rt.executor();
        let mut chain_service = ChainService::new(&exe);

        let state_db = chain_service.state_db.lock().unwrap();
        let s_n = state_db.sequence_number(&account_address).unwrap();
        drop(state_db);
        let signed_tx = RawTransaction::new(
            account_address,
            s_n as u64,
            program,
            1_000_000 as u64,
            1 as u64,
            Duration::from_secs(u64::max_value()),
        ).sign(&GENESIS_KEYPAIR.0, GENESIS_KEYPAIR.1.clone())
            .unwrap()
            .into_inner();

        chain_service.apply_on_chain_transaction(signed_tx);
    }

    #[test]
    fn test_faucet() {
        let mut rt = Runtime::new().unwrap();
        let exe = rt.executor();
        let mut chain_service = ChainService::new(&exe);
        let receiver = AccountAddress::random();
        chain_service.faucet_inner(receiver, 100);
        let state_db = chain_service.state_db.lock().unwrap();
        let exist_flag = state_db.exist_account(&receiver);
        assert_eq!(exist_flag, true);
    }

    #[test]
    fn test_account_state_proof() {
        let mut rt = Runtime::new().unwrap();
        let exe = rt.executor();
        let mut chain_service = ChainService::new(&exe);
        let mut query_addr: AccountAddress = AccountAddress::random();
        for i in 1..10 {
            let receiver = AccountAddress::random();
            if i == 5 {
                query_addr = receiver.clone();
            }
            chain_service.faucet_inner(receiver, 100);
        }

        let proof = chain_service.get_account_state_with_proof_inner(&query_addr, Some(8));
        match proof {
            Some((v, a, b)) => {
                println!("{:?}", query_addr.hash());
                println!("{:?}", b);
            }
            None => {}
        }
    }
}
