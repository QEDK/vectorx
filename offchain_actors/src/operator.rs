
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;
use std::time::{SystemTime, Duration};

use avail_subxt::AvailConfig;
use base58::FromBase58;
use futures::future::Fuse;
use plonky2::gates::public_input;
use service::{ProofGeneratorClient, to_bits};
use avail_subxt::{api, build_client, primitives::Header};
use codec::{Decode, Encode};
use pallet_grandpa::{VersionedAuthorityList, AuthorityList, AuthorityId};
use plonky2lib_succinct::ed25519::curve::eddsa::{EDDSASignature, verify_message, EDDSAPublicKey};
use plonky2lib_succinct::ed25519::field::ed25519_scalar::Ed25519Scalar;
use plonky2lib_succinct::ed25519::gadgets::curve::decompress_point;
use num::BigUint;
use plonky2_field::types::Field;
use serde::de::Error;
use serde::Deserialize;

use sp_core::{
	bytes,
	ed25519::{self, Public as EdPublic, Signature},
	Pair, H256,
};
use subxt::config::{Hasher, Header as SPHeader};
use subxt::OnlineClient;
use subxt::rpc::{RpcParams, Subscription};

use tarpc::{client, context};
use tarpc::tokio_serde::formats::Json;

use futures::{select, StreamExt, pin_mut};


#[derive(Deserialize, Debug)]
pub struct SubscriptionMessageResult {
    pub result: String,
    pub subscription: String,
}

#[derive(Deserialize, Debug)]
pub struct SubscriptionMessage {
    pub jsonrpc: String,
    pub params: SubscriptionMessageResult,
    pub method: String,
}

#[derive(Clone, Debug, Decode, Encode, Deserialize)]
pub struct Precommit {
    pub target_hash: H256,
    /// The target block's number
    pub target_number: u32,
}

#[derive(Clone, Debug, Decode, Deserialize)]
pub struct SignedPrecommit {
    pub precommit: Precommit,
    /// The signature on the message.
    pub signature: Signature,
    /// The Id of the signer.
    pub id: EdPublic,
}
#[derive(Clone, Debug, Decode, Deserialize)]
pub struct Commit {
    pub target_hash: H256,
    /// The target block's number.
    pub target_number: u32,
    /// Precommits for target block or any block after it that justify this commit.
    pub precommits: Vec<SignedPrecommit>,
}

#[derive(Clone, Debug, Decode)]
pub struct GrandpaJustification {
    pub round: u64,
    pub commit: Commit,
    pub votes_ancestries: Vec<Header>,
}

impl<'de> Deserialize<'de> for GrandpaJustification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = bytes::deserialize(deserializer)?;
        Self::decode(&mut &encoded[..])
            .map_err(|codec_err| D::Error::custom(format!("Invalid decoding: {:?}", codec_err)))
    }
}

#[derive(Debug, Decode)]
pub struct Authority(EdPublic, u64);

#[derive(Debug, Encode)]
pub enum SignerMessage {
    DummyMessage(u32),
    PrecommitMessage(Precommit),
}

async fn get_authority_set(c: &OnlineClient<AvailConfig>, block_hash: H256) -> (Vec<Vec<u8>>, Vec<u8>) {
    let grandpa_authorities_bytes = c.storage().at(Some(block_hash)).await.unwrap().fetch_raw(b":grandpa_authorities").await.unwrap().unwrap();
    let grandpa_authorities = VersionedAuthorityList::decode(&mut grandpa_authorities_bytes.as_slice()).unwrap();
    let authority_list:AuthorityList = grandpa_authorities.into();

    let decoded_authority_set = authority_list.iter()
        .map(|authority|
            {
                let auth_bytes = authority.0.to_string().from_base58().unwrap();
                auth_bytes.as_slice()[1..33].to_vec()
            }
        )
        .collect::<Vec<_>>();
    let hash_input = decoded_authority_set.clone().into_iter().flatten().collect::<Vec<_>>();
    let authority_set_commitment = avail_subxt::config::substrate::BlakeTwo256::hash(&hash_input);

    (decoded_authority_set, authority_set_commitment.as_bytes().to_vec())
}

async fn submit_proof_gen_request(
    head_block_num: u32,
    head_block_hash: H256,
    headers: Vec<Header>,
    justification: GrandpaJustification,
    authority_set_id: u64,
    authority_set: Vec<Vec<u8>>,
    authority_set_commitment: Vec<u8>,
) {
    println!("Generate justification for block number: {:?}", justification.commit.target_number);

    // First scale encode the headers
    let encoded_headers = headers.iter().map(|x| x.encode()).collect::<Vec<_>>();

    // Form a message which is signed in the justification
    let precommit_message = Encode::encode(&(
        &SignerMessage::PrecommitMessage(justification.commit.precommits[0].clone().precommit),
        &justification.round,
        &authority_set_id,
    ));

    let signatures = justification.
        commit.
        precommits.
        iter().
        map(|x| x.clone().signature.0).collect::<Vec<_>>();


    let public_keys = justification.
        commit.
        precommits.
        iter().
        map(|x| x.clone().id.0).collect::<Vec<_>>();

    println!("public_keys: {:?}", public_keys);
    println!("authority_set: {:?}", authority_set);

    // Find the pub_key_indices
    let pub_key_indices = public_keys.iter()
        .map(|x| authority_set.iter().position(|y| y == x)
        .unwrap()).collect::<Vec<_>>();

    let mut context = context::current();
    context.deadline = SystemTime::now() + Duration::from_secs(600);

    // Convert signatures to Vec<Vec<u8>>
    let sigs = signatures.iter().map(|x| x.to_vec()).collect::<Vec<_>>();


    /*
    headers: Vec<Vec<u8>>,
    head_block_hash: Vec<u8>,
    head_block_num: u64,

    authority_set_id: u64,
    precommit_message: Vec<u8>,
    signatures: Vec<Vec<u8>>,

    pub_key_indices: Vec<usize>,
    authority_set: Vec<Vec<u8>>,
    authority_set_commitment: Vec<u8>,
    */

    println!("headers: {:?}", encoded_headers);
    println!("head_block_hash: {:?}", head_block_hash);
    println!("head_block_num: {:?}", head_block_num);
    println!("authority_set_id: {:?}", authority_set_id);
    println!("precommit_message: {:?}", precommit_message);
    println!("signatures: {:?}", sigs);
    println!("pub_key_indices: {:?}", pub_key_indices);
    println!("authority_set: {:?}", authority_set);
    println!("authority_set_commitment: {:?}", authority_set_commitment);

    /*

    let res = client.generate_grandpa_justif_proof(
        context, 
        block_hash, 
        encoded_header.clone(),
        encoded_message.clone(),
        sigs.clone(),
        sig_owners.clone(),
    ).await;
        
    match res {
        Ok(_) => println!("Retrieved grandpa justification verification proof for block: number - {:?}; hash - {:?}", header.number, block_hash),
        Err(e) => println!("{:?}", anyhow::Error::from(e)),
    }
    */

    println!("\n\n\n");
}


async fn main_loop(
    header_sub : Subscription<Header>,
    justification_sub : Subscription<GrandpaJustification>,
    c: OnlineClient<AvailConfig>,
) {
    let fused_header_sub = header_sub.fuse();
    let fused_justification_sub = justification_sub.fuse();

    pin_mut!(fused_header_sub, fused_justification_sub);

    let mut last_processed_block_num: Option<u32> = None;
    let mut last_processed_block_hash: Option<H256> = None;
    let mut headers = HashMap::new();

    // If this is not none, then the main loop will submit a proof generation request
    let mut justification_to_process = None;

    'main_loop: loop {
        select! {
            // Currently assuming that all the headers received will be sequential
            header = fused_header_sub.next() => {
                let unwrapped_header = header.unwrap().unwrap();

                if last_processed_block_num.is_none() {
                    last_processed_block_num = Some(unwrapped_header.number);
                    last_processed_block_hash = Some(unwrapped_header.hash());
                }

                println!("Downloaded a header for block number: {:?}", unwrapped_header.number);
                headers.insert(unwrapped_header.number, unwrapped_header);

                // TODO: Handle rotations if there is a new grandpa authority set event in the downloaded header
            }

            justification = fused_justification_sub.next() => {
                // Wait until we get at least one header
                if last_processed_block_num.is_none() {
                    continue;
                }

                let unwrapped_just = justification.unwrap().unwrap();

                if justification_to_process.is_none() && unwrapped_just.commit.target_number >= last_processed_block_num.unwrap() + 5 {
                    println!("Saving justification for block number: {:?}", unwrapped_just.commit.target_number);
                    justification_to_process = Some(unwrapped_just);
                }
            }
        }

        if justification_to_process.is_some() {
            let unwrapped_just = justification_to_process.clone().unwrap();

            let just_block_num = unwrapped_just.commit.target_number;

            // Check to see if we downloaded the header yet
            if !headers.contains_key(&just_block_num) {
                println!("Don't have header for block number: {:?}", just_block_num);
                continue 'main_loop;
            }

            // Check that all the precommit's target number is the same as the precommits' target number
            for precommit in unwrapped_just.commit.precommits.iter() {
                if just_block_num != precommit.precommit.target_number {
                    println!(
                        "Justification has precommits that are not the same number as the commit. Commit's number: {:?}, Precommit's number: {:?}",
                        just_block_num,
                        precommit.precommit.target_number
                    );
                    justification_to_process = None;
                    continue 'main_loop;
                }
            }

            let set_id_key = api::storage().grandpa().current_set_id();

            // Need to get the set id at the previous block
            let previous_hash: H256 = headers.get(&(just_block_num)).unwrap().parent_hash;
            let set_id = c.storage().at(Some(previous_hash)).await.unwrap().fetch(&set_id_key).await.unwrap().unwrap();
            let (authority_set, authority_set_commitment) = get_authority_set(&c, previous_hash).await;

            // Form a message which is signed in the justification
            let signed_message = Encode::encode(&(
                &SignerMessage::PrecommitMessage(unwrapped_just.commit.precommits[0].clone().precommit),
                &unwrapped_just.round,
                &set_id,
            ));

            // Verify all the signatures of the justification and extract the public keys
            for precommit in unwrapped_just.commit.precommits.iter() {
                let is_ok = <ed25519::Pair as Pair>::verify_weak(
                    &precommit.clone().signature.0[..],
                    signed_message.as_slice(),
                    precommit.clone().id,
                );
                if !is_ok {
                    println!("Invalid signature in justification");
                    justification_to_process = None;
                    continue 'main_loop;
                }
            }

            let mut header_batch = Vec::new();
            if headers.contains_key(&unwrapped_just.commit.target_number) {
                for i in last_processed_block_num.unwrap()+1..unwrapped_just.commit.target_number+1 {
                    header_batch.push(headers.get(&i).unwrap().clone());
                    headers.remove(&i);
                }
            }

            println!(
                "Going to process a batch of headers of size: {:?}, block numbers: {:?} and justification with number {:?}",
                header_batch.len(),
                header_batch.iter().map(|h| h.number).collect::<Vec<u32>>(),
                unwrapped_just.commit.target_number,
            );

            submit_proof_gen_request(
                last_processed_block_num.unwrap(),
                last_processed_block_hash.unwrap(),
                header_batch,
                justification_to_process.unwrap(),
                set_id,
                authority_set,
                authority_set_commitment,
            ).await;

            last_processed_block_num = Some(unwrapped_just.commit.target_number);
            last_processed_block_hash = Some(unwrapped_just.commit.target_hash);
            justification_to_process = None;
        }
    }
}

#[tokio::main]
pub async fn main() {
    /*
    let server_addr = (IpAddr::V6(Ipv6Addr::LOCALHOST), 52357);

    let mut transport = tarpc::serde_transport::tcp::connect(server_addr, Json::default);
    transport.config_mut().max_frame_length(usize::MAX);

    let client = ProofGeneratorClient::new(client::Config::default(), transport.await?).spawn();
    */

    let url: &str = "wss://kate.avail.tools:443/ws";
    
    let c = build_client(url, false).await.unwrap();
    let t = c.rpc();

    // TODO:  Will need to sync the chain first

    let header_sub: subxt::rpc::Subscription<Header> = t
    .subscribe(
        "chain_subscribeFinalizedHeads",
        RpcParams::new(),
        "chain_unsubscribeFinalizedHeads",
    )
    .await
    .unwrap();

    let justification_sub: subxt::rpc::Subscription<GrandpaJustification> = t
        .subscribe(
            "grandpa_subscribeJustifications",
            RpcParams::new(),
            "grandpa_unsubscribeJustifications",
        )
        .await
        .unwrap();

    main_loop(header_sub, justification_sub, c).await;
}
