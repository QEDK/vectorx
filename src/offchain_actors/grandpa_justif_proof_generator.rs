
use std::time::SystemTime;

use avail_proof_generators::gadgets::consensus::{GrandpaJustificationVerifierTargets, build_grandpa_justification_verifier};
use avail_subxt::{api, build_client, primitives::Header};
use codec::{Decode, Encode};
use ::ed25519::curve::ed25519::Ed25519;
use ::ed25519::curve::eddsa::{EDDSASignature, verify_message, EDDSAPublicKey};
use ::ed25519::field::ed25519_scalar::Ed25519Scalar;
use ::ed25519::gadgets::curve::{decompress_point, WitnessAffinePoint};
use ::ed25519::gadgets::nonnative::WitnessNonNative;
use num::BigUint;
use plonky2::iop::witness::{PartialWitness, Witness};
use plonky2::plonk::circuit_builder::CircuitBuilder;
use plonky2::plonk::circuit_data::{CircuitData, CircuitConfig};
use plonky2::plonk::config::{PoseidonGoldilocksConfig, GenericConfig};
use plonky2::plonk::proof::ProofWithPublicInputs;
use plonky2_field::goldilocks_field::GoldilocksField;
use plonky2_field::types::Field;
use serde::de::Error;
use serde::Deserialize;

use subxt::{
	ext::{
		sp_core::{blake2_256, bytes, crypto::Pair, ed25519::{self, Public as EdPublic, Signature}, H256},
	},
    rpc::RpcParams,
};


const D: usize = 2;
type C = PoseidonGoldilocksConfig;
type F = <C as GenericConfig<D>>::F;
type Curve = Ed25519;

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

pub const CHUNK_128_BYTES: usize = 128;

fn to_bits(msg: Vec<u8>) -> Vec<bool> {
    let mut res = Vec::new();
    for i in 0..msg.len() {
        let char = msg[i];
        for j in 0..8 {
            if (char & (1 << 7 - j)) != 0 {
                res.push(true);
            } else {
                res.push(false);
            }
        }
    }
    res
}

fn generate_proof(
    granda_justif_circuit: &CircuitData<F, C, D>,
    encoded_header: Vec<u8>,
    encoded_message: Vec<u8>,
    signatures: Vec<[u8; 64]>,
    pub_keys: Vec<[u8; 32]>,
    targets: GrandpaJustificationVerifierTargets<Curve>
) -> Option<ProofWithPublicInputs<F, C, D>> {
    let mut pw: PartialWitness<F> = PartialWitness::new();

    for i in 0..encoded_header.len() {
        pw.set_target(targets.encoded_header[i], GoldilocksField(encoded_header[i] as u64));
    }
    for i in encoded_header.len() .. CHUNK_128_BYTES * 10 {
        pw.set_target(targets.encoded_header[i], GoldilocksField(0));
    }

    pw.set_target(targets.encoded_header_length, GoldilocksField(encoded_header.len() as u64));

    for i in 0..encoded_message.len() {
        pw.set_target(targets.encoded_message[i], GoldilocksField(encoded_message[i] as u64));
    }

    let encoded_messsage_bits = to_bits(encoded_message.to_vec());

    // We are hardcoding verifition of 7 signatures for now.
    // Avail testnet has 10 validators, so a quorum [ceil(2/3*n)] is 7.
    for i in 0..7 {
        let sig_r = decompress_point(&signatures[i][0..32]);
        assert!(sig_r.is_valid());

        let sig_s_biguint = BigUint::from_bytes_le(&signatures[i][32..64]);
        let sig_s = Ed25519Scalar::from_noncanonical_biguint(sig_s_biguint);
        let sig = EDDSASignature { r: sig_r, s: sig_s };

        let pub_key = decompress_point(&pub_keys[i][..]);
        assert!(pub_key.is_valid());

        assert!(verify_message(
            &encoded_messsage_bits,
            &sig,
            &EDDSAPublicKey(pub_key)
        ));

        // eddsa verification witness stuff
        pw.set_affine_point_target(&targets.pub_keys[i].0, &pub_key);
        pw.set_affine_point_target(&targets.signatures[i].r, &sig_r);
        pw.set_nonnative_target(&targets.signatures[i].s, &sig_s);
    }

    let proof = granda_justif_circuit.prove(pw);

    match proof {
        Ok(v) => return Some(v),
        Err(e) => println!("error parsing header: {e:?}"),
    };

    return None
}

#[tokio::main]
pub async fn main() {
    // Compile the header validation circuit
    const CHUNK_128_BYTES:usize = 128;

    let mut builder = CircuitBuilder::<F, D>::new(CircuitConfig::standard_ecc_config());
    let targets = build_grandpa_justification_verifier::<GoldilocksField, Curve, D>(&mut builder, CHUNK_128_BYTES * 10, 7);
    let grandpa_justif_circuit = builder.build::<C>();

    let url: &str = "wss://testnet.avail.tools:443/ws";
    
    let c = build_client(url).await.unwrap();
    let t = c.rpc();
    let sub: Result<subxt::rpc::Subscription<GrandpaJustification>, subxt::Error> = t
        .subscribe(
            "grandpa_subscribeJustifications",
            RpcParams::new(),
            "grandpa_unsubscribeJustifications",
        )
        .await;

    let mut sub = sub.unwrap();

    // How often we want to generate a proof of grandpa justification
    const FINALIZATION_PERIOD: usize = 20;

    // Wait for headers
    while let Some(Ok(justification)) = sub.next().await {
        // Get the header corresponding to the new justification
        let header = c
            .rpc()
            .header(Some(justification.commit.target_hash))
            .await
            .unwrap()
            .unwrap();

        println!("Got justification for header with number: {:?}", header.number);

        if header.number % (FINALIZATION_PERIOD as u32) == 0 {
            let block_hash: H256 = Encode::using_encoded(&header, blake2_256).into();
            println!("Generate justification for header with number: {:?} and hash: {:?}", header.number, block_hash);

            // Get current authority set ID
            let set_id_key = api::storage().grandpa().current_set_id();
            let set_id = c.storage().fetch(&set_id_key, None).await.unwrap().unwrap();

            // Form a message which is signed in the justification
            let encoded_message = Encode::encode(&(
                &SignerMessage::PrecommitMessage(justification.commit.precommits[0].clone().precommit),
                &justification.round,
                &set_id,
            ));

            let signatures = justification.
            commit.
            precommits.
            iter().
            map(|x| x.clone().signature.0).collect::<Vec<_>>();

            let sig_owners = justification
            .commit
            .precommits
            .iter()
            .map(|precommit| {
                let is_ok = <ed25519::Pair as Pair>::verify_weak(
                    &precommit.clone().signature.0[..],
                    encoded_message.as_slice(),
                    &precommit.clone().id,
                );
                assert!(is_ok, "Not signed by this signature!");
                assert!(precommit.signature.0.len() == 64);
                assert!(precommit.id.0.len() == 32);
                precommit.clone().id.0
            })
            .collect::<Vec<_>>();

            // retrieve the signatures
            let encoded_messsage_bits = to_bits(encoded_message.clone());

            for i in 0..signatures.len() {
                let sig_r = decompress_point(&signatures[i][0..32]);
                assert!(sig_r.is_valid());
        
                let sig_s_biguint = BigUint::from_bytes_le(&signatures[i][32..64]);
                let sig_s = Ed25519Scalar::from_noncanonical_biguint(sig_s_biguint);
                let sig = EDDSASignature { r: sig_r, s: sig_s };
        
                let pub_key = decompress_point(&sig_owners[i][0..32]);
                assert!(pub_key.is_valid());
        
                assert!(verify_message(
                    &encoded_messsage_bits,
                    &sig,
                    &EDDSAPublicKey(pub_key)
                ));
            }

            let encoded_header = header.encode();
            let proof_gen_start_time = SystemTime::now();
            let proof = generate_proof(
                &grandpa_justif_circuit,
                encoded_header,
                encoded_message,
                signatures,
                sig_owners,
                targets.clone()
            );
            let proof_gen_end_time = SystemTime::now();
            let proof_gen_duration = proof_gen_end_time.duration_since(proof_gen_start_time).unwrap();    
            if proof.is_some() {
                println!("generated proof.  proof gen time is {:?}", proof_gen_duration);

                let proof_verification_start_time = SystemTime::now();
                let verification_res = grandpa_justif_circuit.verify(proof.unwrap());
                let proof_verification_end_time = SystemTime::now();
                let proof_verification_time = proof_verification_end_time.duration_since(proof_verification_start_time).unwrap();
                println!("proof generated - time: {:?}", proof_verification_time);

                if !verification_res.is_err() {
                    println!("proof verification succeeded");
                } else {
                    println!("proof verification failed");
                }
            } else {
                println!("failed to generate proof");
            }
            println!("\n\n\n");
        }
    }
}
