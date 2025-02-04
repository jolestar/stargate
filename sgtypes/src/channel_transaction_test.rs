// Copyright (c) The Starcoin Core Contributors
// SPDX-License-Identifier: Apache-2.0

use canonical_serialization::{
    CanonicalDeserializer, CanonicalSerializer, SimpleDeserializer, SimpleSerializer,
};
use crypto::{
    ed25519::{Ed25519PrivateKey, Ed25519PublicKey},
    test_utils::KeyPair,
    Uniform,
};
use libra_types::{
    account_address::AccountAddress,
    transaction::{ChannelScriptPayload, ChannelWriteSetPayload, Script},
    transaction_helpers::ChannelPayloadSigner,
    write_set::WriteSet,
};
use rand::prelude::*;

use crate::channel_transaction::{
    ChannelOp, ChannelTransactionRequest, ChannelTransactionRequestPayload, Witness,
};
use failure::_core::time::Duration;

//TODO(jole) use Arbitrary
#[test]
fn request_roundtrip_canonical_serialization() {
    let mut rng0: StdRng = SeedableRng::from_seed([0; 32]);
    let keypair: KeyPair<Ed25519PrivateKey, Ed25519PublicKey> =
        KeyPair::generate_for_testing(&mut rng0);
    let sender = AccountAddress::from_public_key(&keypair.public_key);
    let receiver = AccountAddress::random();
    let script = Script::new(vec![], vec![]);
    let channel_script_payload =
        ChannelScriptPayload::new(0, WriteSet::default(), receiver, script);
    let signature = keypair
        .sign_script_payload(&channel_script_payload)
        .unwrap();
    let sequence_number = rng0.next_u64();
    let channel_sequence_number = rng0.next_u64();

    let requests = vec![
        ChannelTransactionRequest::new(
            rng0.next_u64(),
            ChannelOp::Open,
            sender,
            sequence_number,
            receiver,
            channel_sequence_number,
            Duration::from_secs(rng0.next_u64()),
            ChannelTransactionRequestPayload::Offchain(Witness {
                witness_payload: ChannelWriteSetPayload::new(
                    channel_sequence_number,
                    WriteSet::default(),
                    receiver,
                ),
                witness_signature: signature.clone(),
            }),
            keypair.public_key.clone(),
            Vec::new(),
        ),
        ChannelTransactionRequest::new(
            rng0.next_u64(),
            ChannelOp::Execute {
                package_name: "Test".to_string(),
                script_name: "Test".to_string(),
            },
            sender,
            sequence_number,
            receiver,
            channel_sequence_number,
            Duration::from_secs(rng0.next_u64()),
            ChannelTransactionRequestPayload::Travel {
                max_gas_amount: rng0.next_u64(),
                gas_unit_price: rng0.next_u64(),
                txn_write_set_hash: Default::default(),
                txn_signature: signature,
            },
            keypair.public_key.clone(),
            Vec::new(),
        ),
    ];
    for request in requests {
        let mut serializer = SimpleSerializer::<Vec<u8>>::new();
        serializer.encode_struct(&request).unwrap();
        let serialized_bytes = serializer.get_output();

        let mut deserializer = SimpleDeserializer::new(&serialized_bytes);
        let output: ChannelTransactionRequest = deserializer.decode_struct().unwrap();
        assert_eq!(request, output);
    }
}
