use ed25519::curve::curve_types::Curve;
use ed25519::gadgets::eddsa::verify_message_circuit;
use ed25519::gadgets::eddsa::{ EDDSATargets, EDDSASignatureTarget, EDDSAPublicKeyTarget };
use ed25519::sha512::blake2b::{make_blake2b_circuit, CHUNK_128_BYTES};
use ed25519::sha512::blake2b::Blake2bTarget;
use plonky2::hash::hash_types::RichField;
use plonky2::plonk::circuit_builder::CircuitBuilder;
use plonky2::plonk::plonk_common::reduce_with_powers_circuit;
use plonky2_field::extension::Extendable;
use plonky2::iop::target::Target;

use crate::encoding::make_scale_header_circuit;

pub struct SignedPrecommitTarget<C: Curve> {
    pub block_hash: Blake2bTarget,
    pub block_num: Target,
    pub eddsa_sig: EDDSATargets<C>,
}

pub struct GrandpaJustificationTarget<C: Curve> {
    pub round: Target,     // Technically, this field is a u64, and a target is slightly smaller.  But I think its a safe value for now.
    pub signed_precommits: Vec<SignedPrecommitTarget<C>>
}

#[derive(Clone)]
pub struct GrandpaJustificationVerifierTargets<C: Curve> {
    pub encoded_header: Vec<Target>,
    pub encoded_header_length: Target,
    pub encoded_message: Vec<Target>, // Encoded message is 53 bytes.
    pub signatures: Vec<EDDSASignatureTarget<C>>,
    pub pub_keys: Vec<EDDSAPublicKeyTarget<C>>
}

const ENCODED_MESSAGE_LENGTH: usize = 53;

pub fn build_grandpa_justification_verifier<F: RichField + Extendable<D>, C: Curve, const D: usize>(
    builder: &mut CircuitBuilder<F, D>,
    max_encoded_header_length: usize,   // in bytes
    num_validators: usize
) -> GrandpaJustificationVerifierTargets<C> {
    let mut encoded_header = Vec::with_capacity(max_encoded_header_length as usize);
    for _i in 0..max_encoded_header_length {
        encoded_header.push(builder.add_virtual_target());
    }

    // Range check the encoded header elements.  Should be between 0 - 255 (inclusive)
    for i in 0..max_encoded_header_length {
        builder.range_check(encoded_header[i], 8);
    }

    let scale_header_decoder = make_scale_header_circuit(builder, max_encoded_header_length);
    let scale_header_deocder_input = scale_header_decoder.get_encoded_header_target();

    for i in 0..max_encoded_header_length {
        builder.connect(encoded_header[i], scale_header_deocder_input[i]);
    }

    let blake2_target = make_blake2b_circuit(
        builder, 
        CHUNK_128_BYTES * 8 * 10, 
        32
    );  // 32 bytes = 256 bits
    for i in 0..max_encoded_header_length {
        let mut bits = builder.split_le(encoded_header[i], 8);
        // want the bits in big endian order
        bits.reverse();
        for j in 0..8 {
            builder.connect(bits[j].target, blake2_target.message[i * 8 + j].target);
        }
    }

    let encoded_header_length_target = builder.add_virtual_target();
    builder.connect(blake2_target.message_len, encoded_header_length_target);

    let mut encoded_message = Vec::with_capacity(ENCODED_MESSAGE_LENGTH);
    for _i in 0..ENCODED_MESSAGE_LENGTH {
        encoded_message.push(builder.add_virtual_target());
    }

    // Range check the encoded message elements
    for i in 0..ENCODED_MESSAGE_LENGTH {
        builder.range_check(encoded_message[i], 8);
    }

    // Check to make sure the encoded message is correct.
    // First byte should have a value of 1
    let one = builder.constant(F::from_canonical_u8(1));
    builder.connect(encoded_message[0], one);

    // The next 32 bytes of the encoded message should equal to the blake2_target digest.
    // Note that the blake2_target digest is in bits where the bit ordering is big endian.
    // We will need to reverse that bit ordering when calculating the byte value, since the
    // we are using the builder's le_sum function (input is little endian) to calculate the byte value.
    for i in 0..32 {
        // Get 8 bit chunk of the blake2_target digest
        let digest_bit_chunk = blake2_target.digest[i * 8..(i + 1) * 8].to_vec();
        let digest_byte = builder.le_sum(digest_bit_chunk.iter().rev());

        builder.connect(digest_byte, encoded_message[i + 1]);
    }

    // The next 4 bytes of the encoded message should equal to the block number in little endian byte order
    let block_num = scale_header_decoder.get_number(builder);
    let alpha = builder.constant(F::from_canonical_u16(256));
    let encoded_message_block_num = reduce_with_powers_circuit(builder, &encoded_message[33..37], alpha);
    builder.connect(encoded_message_block_num, block_num);


    // Need to convert the encoded message to a bit array.  For now, assume that all validators are signing the same message
    let mut encoded_msg_bits = Vec::with_capacity(ENCODED_MESSAGE_LENGTH * 8);
    for i in 0..ENCODED_MESSAGE_LENGTH {
        let mut bits = builder.split_le(encoded_message[i], 8);

        // Needs to be in bit big endian order for the EDDSA verification circuit
        bits.reverse();
        for j in 0..8 {
            encoded_msg_bits.push(bits[j]);
        }
    }

    let mut signatures = Vec::with_capacity(num_validators);
    let mut pub_keys = Vec::with_capacity(num_validators);
    for _i in 0..num_validators {
        let eddsa_verify_circuit = verify_message_circuit(builder, ENCODED_MESSAGE_LENGTH as u128);

        for j in 0..ENCODED_MESSAGE_LENGTH * 8 {
            builder.connect(encoded_msg_bits[j].target, eddsa_verify_circuit.msg[j].target);
        }

        signatures.push(eddsa_verify_circuit.sig);
        pub_keys.push(eddsa_verify_circuit.pub_key);
    }

    GrandpaJustificationVerifierTargets {
        encoded_header: encoded_header,
        encoded_header_length: encoded_header_length_target,
        encoded_message: encoded_message,
        signatures: signatures,
        pub_keys: pub_keys
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use anyhow::Result;
    use ed25519::curve::ed25519::Ed25519;
    use ed25519::curve::eddsa::{verify_message, EDDSAPublicKey, EDDSASignature};
    use ed25519::field::ed25519_scalar::Ed25519Scalar;
    use ed25519::gadgets::curve::{decompress_point, WitnessAffinePoint};
    use ed25519::gadgets::eddsa::verify_message_circuit;
    use ed25519::gadgets::nonnative::WitnessNonNative;
    use ed25519::sha512::blake2b::{ make_blake2b_circuit, CHUNK_128_BYTES };
    use ed25519_dalek::{PublicKey, Signature};
    use hex::decode;
    use num::BigUint;
    use plonky2::iop::witness::{PartialWitness, Witness};
    use plonky2::plonk::circuit_builder::CircuitBuilder;
    use plonky2::plonk::circuit_data::CircuitConfig;
    use plonky2::plonk::config::{GenericConfig, PoseidonGoldilocksConfig};
    use plonky2_field::goldilocks_field::GoldilocksField;
    use plonky2_field::types::Field;

    use crate::consensus::build_grandpa_justification_verifier;

    #[test]
    fn test_avail_eddsa_circuit() -> Result<()> {
        const D: usize = 2;
        type C = PoseidonGoldilocksConfig;
        type F = <C as GenericConfig<D>>::F;

        let mut pw = PartialWitness::new();
        let mut builder = CircuitBuilder::<F, D>::new(CircuitConfig::standard_ecc_config());

        // grabbed from avail round 7642
        // let sig_msg = hex::decode("019f87ee2f28b0debfbe661c573beaa899db45211fd2b3b41f44bdbc685599e2be106a040082300000000000000f01000000000000").unwrap();
        let sig_msg = hex::decode("0162f1aaf6297b86b3749448d66cc43deada49940c3912a4ec4916344058e8f0655f180800680b000000000000f001000000000000").unwrap();
        let sig_msg_bits = to_bits(sig_msg.to_vec());

        // let signature= hex::decode("cc1286a1716d00a8c310cfae32e8fb83f5f9c8bc1a6da1fb0de33361aaf7871d2e33e6c72606295e5b175ec4033b7e63392d76e2e4271561d6d2db149191e609").unwrap();
        let signature= hex::decode("3ebc508daaf5edd7a4b4779743ce9241519aa8940264c2be4f39dfd0f7a4f2c4c587752fbc35d6d34b8ecd494dfe101e49e6c1ccb0e41ff2aa52bc481fcd3e0c").unwrap();
        let sig_r = decompress_point(&signature[0..32]);
        assert!(sig_r.is_valid());

        let sig_s_biguint =
            BigUint::from_bytes_le(&signature[32..64]);
        let sig_s = Ed25519Scalar::from_noncanonical_biguint(sig_s_biguint);
        let sig = EDDSASignature { r: sig_r, s: sig_s };

        let pubkey_bytes =
            hex::decode("0e0945b2628f5c3b4e2a6b53df997fc693344af985b11e3054f36a384cc4114b")
                .unwrap();
        let pub_key = decompress_point(&pubkey_bytes[..]);
        assert!(pub_key.is_valid());

        assert!(verify_message(
            &sig_msg_bits,
            &sig,
            &EDDSAPublicKey(pub_key)
        ));

        let targets = verify_message_circuit(&mut builder, sig_msg.len().try_into().unwrap());

        let mut msg = Vec::with_capacity(sig_msg.len());
        for i in 0..sig_msg.len() {
            msg.push(CircuitBuilder::constant(&mut builder, F::from_canonical_u8(sig_msg[i])));
        }

        let mut msg_bits = Vec::with_capacity(sig_msg.len() * 8);
        for i in 0..sig_msg.len() {
            let mut bits = builder.split_le(msg[i], 8);

            // Needs to be in bit big endian order for the EDDSA verification circuit
            bits.reverse();
            for j in 0..8 {
                msg_bits.push(bits[j]);
            }
        }

        for i in 0..msg_bits.len() {
            builder.connect(msg_bits[i].target, targets.msg[i].target)
        }

        pw.set_affine_point_target(&targets.pub_key.0, &pub_key);
        pw.set_affine_point_target(&targets.sig.r, &sig_r);
        pw.set_nonnative_target(&targets.sig.s, &sig_s);

        let data = builder.build::<C>();
        let proof = data.prove(pw).unwrap();
        data.verify(proof)
    }

    #[test]
    fn test_avail_eddsa() -> Result<()> {
        let sig_msg = hex::decode("0162f1aaf6297b86b3749448d66cc43deada49940c3912a4ec4916344058e8f0655f180800680b000000000000f001000000000000").unwrap();

        let signature_bytes= hex::decode("3ebc508daaf5edd7a4b4779743ce9241519aa8940264c2be4f39dfd0f7a4f2c4c587752fbc35d6d34b8ecd494dfe101e49e6c1ccb0e41ff2aa52bc481fcd3e0c").unwrap();
        let signature = Signature::from_bytes(&signature_bytes)?;

        let pubkey_bytes =
            hex::decode("0e0945b2628f5c3b4e2a6b53df997fc693344af985b11e3054f36a384cc4114b")
                .unwrap();
        let pubkey = PublicKey::from_bytes(&pubkey_bytes)?;

        let is_ok = PublicKey::verify_strict(&pubkey, &sig_msg[..], &signature);

        if is_ok.is_err() {
            panic!("it dont work");
        }
        Ok(())
    }

    #[test]
    fn test_blake2() -> Result<()> {
        // let hash_msg = [144, 117, 45, 72, 111, 237, 228, 218, 116, 101, 43, 1, 223, 40, 157, 214, 121, 105, 103, 193, 192, 190, 76, 74, 170, 58, 227, 134, 92, 62, 213, 14, 122, 66, 19, 0, 193, 241, 98, 251, 119, 243, 126, 139, 222, 180, 26, 156, 8, 112, 238, 97, 139, 84, 14, 237, 239, 199, 22, 202, 25, 78, 9, 79, 47, 79, 93, 183, 76, 136, 103, 4, 210, 247, 12, 241, 84, 31, 81, 154, 95, 173, 53, 213, 1, 66, 97, 126, 17, 163, 170, 125, 57, 151, 215, 23, 43, 22, 65, 199, 8, 6, 66, 65, 66, 69, 181, 1, 1, 2, 0, 0, 0, 147, 174, 254, 4, 0, 0, 0, 0, 50, 26, 21, 229, 88, 9, 151, 22, 46, 53, 8, 34, 225, 248, 32, 112, 71, 251, 168, 47, 18, 216, 70, 137, 123, 19, 123, 22, 186, 45, 246, 39, 230, 1, 21, 94, 11, 77, 192, 130, 12, 119, 209, 134, 62, 196, 151, 102, 220, 219, 16, 134, 212, 68, 110, 223, 212, 25, 194, 204, 49, 221, 102, 0, 129, 249, 20, 6, 179, 67, 166, 223, 157, 127, 37, 184, 248, 77, 109, 234, 32, 249, 82, 210, 26, 246, 98, 254, 14, 198, 20, 149, 72, 164, 18, 3, 5, 66, 65, 66, 69, 1, 1, 12, 98, 1, 92, 64, 109, 0, 15, 225, 90, 161, 6, 20, 185, 110, 4, 128, 114, 49, 74, 151, 255, 76, 51, 7, 242, 243, 244, 106, 96, 115, 12, 31, 144, 167, 82, 234, 157, 223, 169, 32, 31, 174, 135, 250, 158, 181, 247, 236, 69, 172, 154, 45, 87, 248, 253, 7, 21, 112, 210, 5, 59, 69, 131, 0, 4, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 129, 1, 171, 123, 251, 117, 13, 76, 175, 100, 53, 44, 84, 214, 215, 237, 138, 212, 129, 32, 136, 13, 248, 88, 237, 131, 73, 6, 10, 6, 98, 221, 120, 24, 230, 125, 12, 215, 241, 155, 79, 26, 112, 220, 246, 161, 103, 151, 163, 92, 171, 123, 251, 117, 13, 76, 175, 100, 53, 44, 84, 214, 215, 237, 138, 212, 129, 32, 136, 13, 248, 88, 237, 131, 73, 6, 10, 6, 98, 221, 120, 24, 230, 125, 12, 215, 241, 155, 79, 26, 112, 220, 246, 161, 103, 151, 163, 92, 4, 0];
        let hash_msg = [199, 211, 173, 1, 148, 4, 184, 168, 129, 158, 147, 33, 229, 118, 216, 13, 115, 19, 16, 247, 138, 29, 38, 58, 16, 201, 126, 227, 246, 65, 8, 20, 18, 67, 19, 0, 206, 224, 13, 243, 214, 81, 145, 197, 22, 192, 185, 107, 124, 228, 0, 177, 84, 165, 36, 60, 189, 240, 167, 84, 127, 36, 127, 208, 159, 18, 50, 187, 77, 220, 223, 56, 66, 161, 202, 34, 41, 145, 22, 163, 106, 65, 153, 178, 42, 201, 52, 212, 10, 217, 219, 194, 46, 53, 158, 23, 20, 222, 6, 120, 8, 6, 66, 65, 66, 69, 52, 2, 8, 0, 0, 0, 185, 174, 254, 4, 0, 0, 0, 0, 5, 66, 65, 66, 69, 1, 1, 58, 46, 57, 142, 51, 163, 68, 38, 85, 183, 197, 114, 35, 78, 186, 34, 26, 1, 156, 8, 97, 93, 205, 194, 183, 135, 236, 239, 49, 238, 230, 126, 17, 234, 170, 147, 165, 234, 103, 133, 13, 142, 94, 84, 170, 115, 252, 85, 122, 118, 38, 180, 165, 130, 28, 14, 36, 93, 69, 137, 130, 219, 194, 132, 0, 4, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 129, 1, 129, 115, 27, 207, 203, 27, 3, 117, 164, 108, 3, 254, 15, 21, 180, 159, 109, 175, 75, 141, 62, 222, 193, 34, 49, 5, 197, 217, 170, 223, 45, 117, 151, 0, 144, 196, 186, 13, 34, 201, 45, 235, 114, 248, 182, 157, 126, 55, 129, 115, 27, 207, 203, 27, 3, 117, 164, 108, 3, 254, 15, 21, 180, 159, 109, 175, 75, 141, 62, 222, 193, 34, 49, 5, 197, 217, 170, 223, 45, 117, 151, 0, 144, 196, 186, 13, 34, 201, 45, 235, 114, 248, 182, 157, 126, 55, 4, 0];
        let hash_msg_bits = to_bits(hash_msg.to_vec());
        // let expected_hash_digest = b"65616ede4572088aae86f4fe72c91284e48d56b6da61b4e0a2f599a750e531b1";
        let expected_hash_digest = b"4741d5048f282c0459e35d951d81d1adc20ef564e341cd84ee834a683aa40571";
        let hash_digest_bits = to_bits(decode(expected_hash_digest).unwrap());
        let hash_len: usize = 32;

        const D: usize = 2;
        type C = PoseidonGoldilocksConfig;
        type F = <C as GenericConfig<D>>::F;
        let mut builder = CircuitBuilder::<F, D>::new(CircuitConfig::standard_recursion_config());
        let targets = make_blake2b_circuit(
            &mut builder,
            CHUNK_128_BYTES * 8 * 10,
            hash_len
        );

        let mut pw = PartialWitness::new();

        for i in 0..hash_msg_bits.len() {
            pw.set_bool_target(targets.message[i], hash_msg_bits[i]);
        }

        for i in hash_msg_bits.len()..CHUNK_128_BYTES * 8 * 10 {
            pw.set_bool_target(targets.message[i], false);
        }

        pw.set_target(targets.message_len, F::from_canonical_usize(hash_msg.len()));

        for i in 0..hash_digest_bits.len() {
            if hash_digest_bits[i] {
                builder.assert_one(targets.digest[i].target);
            } else {
                builder.assert_zero(targets.digest[i].target);
            }
        }

        let data = builder.build::<C>();
        let proof = data.prove(pw).unwrap();
        data.verify(proof)
    }

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

    #[test]
    fn test_grandpa_verification_simple() -> Result<()> {
        // Circuit inputs start
        let encoded_header = [
            96, 238, 56, 116, 214, 247, 155, 165, 75, 180, 249, 182, 41, 114, 233, 5, 160, 204, 66, 136, 228, 66, 135, 151, 23, 189, 98, 3,
            95, 204, 244, 137, 126, 97, 32, 0, 190, 164, 124, 198, 148, 159, 37, 236, 27, 38, 47, 14, 45, 255, 92, 44, 150, 159, 222, 131, 77,
            242, 223, 82, 2, 83, 60, 239, 59, 240, 100, 159, 96, 27, 46, 203, 51, 252, 140, 150, 191, 182, 131, 236, 137, 101, 220, 13, 234, 16,
            27, 244, 228, 172, 234, 8, 48, 159, 13, 152, 219, 155, 247, 203, 8, 6, 66, 65, 66, 69, 181, 1, 1, 3, 0, 0, 0, 88, 246, 1, 5, 0, 0,
            0, 0, 190, 158, 92, 111, 241, 162, 35, 126, 93, 188, 37, 82, 192, 193, 97, 255, 220, 78, 2, 221, 136, 16, 40, 209, 176, 62, 99, 123,
            241, 3, 171, 74, 153, 142, 189, 223, 96, 214, 248, 72, 130, 18, 61, 206, 59, 90, 74, 134, 243, 152, 158, 147, 17, 32, 50, 17, 123,
            252, 105, 201, 101, 221, 52, 12, 228, 84, 40, 97, 46, 24, 29, 44, 193, 195, 239, 85, 92, 11, 209, 116, 131, 131, 112, 141, 93, 248,
            119, 31, 223, 87, 113, 197, 199, 248, 153, 15, 5, 66, 65, 66, 69, 1, 1, 100, 140, 107, 114, 139, 31, 108, 148, 6, 14, 44, 185, 118,
            71, 240, 246, 107, 200, 22, 228, 193, 191, 22, 86, 191, 215, 245, 172, 170, 103, 161, 33, 231, 34, 30, 180, 220, 197, 119, 187, 76,
            252, 234, 86, 161, 209, 72, 150, 67, 111, 29, 80, 105, 21, 27, 224, 76, 214, 153, 24, 99, 218, 93, 133, 0, 4, 16, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 129, 1, 150, 25, 213, 166, 9, 223, 191, 127, 80, 148,
            89, 6, 174, 43, 165, 238, 147, 157, 55, 253, 47, 218, 243, 92, 25, 160, 242, 210, 121, 252, 26, 22, 232, 127, 72, 52, 89, 70, 79, 112,
            217, 198, 184, 97, 175, 188, 82, 217, 150, 25, 213, 166, 9, 223, 191, 127, 80, 148, 89, 6, 174, 43, 165, 238, 147, 157, 55, 253, 47,
            218, 243, 92, 25, 160, 242, 210, 121, 252, 26, 22, 232, 127, 72, 52, 89, 70, 79, 112, 217, 198, 184, 97, 175, 188, 82, 217, 4, 0];

        let encoded_msg = [
            1, 98, 241, 170, 246, 41, 123, 134, 179, 116, 148, 72, 214, 108, 196, 61, 234, 218, 73, 148, 12, 57, 18, 164, 236, 73, 22, 52, 64,
            88, 232, 240, 101, 95, 24, 8, 0, 104, 11, 0, 0, 0, 0, 0, 0, 240, 1, 0, 0, 0, 0, 0, 0];

        let encoded_msg_bits = to_bits(encoded_msg.to_vec());

        let signatures = vec![
            "3ebc508daaf5edd7a4b4779743ce9241519aa8940264c2be4f39dfd0f7a4f2c4c587752fbc35d6d34b8ecd494dfe101e49e6c1ccb0e41ff2aa52bc481fcd3e0c",
            "48f851a4cb99db770461b3b42e7a055fb4801a2a4d2627691e52d0bb955bc8c6c490b0d04d97365e39b7cffeb4489318f28deddbc0710a57f4d94a726a98df01",
            "cbc199cf5754103a3a52d51795596c1535a8766ea84073d6c064db28fa0a357521dd912516d694813e21d279a72f11b59029bed7671db6b0d2ee0cd68d0ebb0f",
            "8f006a2ac7cd3396d20d2230204e2742fd413bde5c4ad6ad688f01def90ae2b80bcfee0507aedbcc01a389c74f7c5315eadedff800f3ff8d7482c2d8afe47500",
            "d5b234c6268f1d822217ac2a88358d31ec14f8f975b0f5d3f87ada7dd88e87400f11e9aac94cab3c2d1e8d38088cc505e9426f35d07a5ae9f7bb5c33244f160a",
            "da57013e372c8cd4aa7bc6c6112d9404325e8d48fcc02c51ad915a725ee0424c3a54cee03dfe315d91f3e6a576f8134a17b28717485340c9ac1ebfe7fc72360f",
            "b22b809b0249ee4e8d43d3aee1a2f40bd529f9eaaa6493d7ec8198b5c93a15ce1e7d653d2aaf710ebfef4ff5aec8e120faf22776417b3621bf6b9de4af540805"
        ];
        let pub_keys = vec![
            "0e0945b2628f5c3b4e2a6b53df997fc693344af985b11e3054f36a384cc4114b",
            "5568a33085a85e1680b83823c6b4b8a0b51d506748b5d5266dd536e258e18a9d",
            "8916179559464bd193d94b053b250a0edf3da5b61d1f2bf2bf2640930dfd2c0e",
            "8d9b15ea8335270510135b7f7c5ef94e0df70e751d3c5f95fd1aa6d7766929b6",
            "8e9edb840fcf9ce51b9d2e65dcae423aafd03ab5973da8d806207395a26af66e",
            "ba76ee41deca67a1d69113f89e233df3a63e6722ca988163848770f4659eb150",
            "e4c08a068e72a466e2f377e862b5b2ed473c4f0e58d7d265a123ad11fef2a797"
        ];
        // Circuit inputs end

        const D: usize = 2;
        type C = PoseidonGoldilocksConfig;
        type F = <C as GenericConfig<D>>::F;
        type Curve = Ed25519;
        let mut builder = CircuitBuilder::<F, D>::new(CircuitConfig::standard_ecc_config());
        let grandpa_justif_targets = build_grandpa_justification_verifier::<GoldilocksField, Curve, D>(&mut builder, CHUNK_128_BYTES * 10, signatures.len());

        let mut pw = PartialWitness::<GoldilocksField>::new();

        for i in 0..encoded_header.len() {
            pw.set_target(grandpa_justif_targets.encoded_header[i], GoldilocksField(encoded_header[i]));
        }
        for i in encoded_header.len() .. CHUNK_128_BYTES * 10 {
            pw.set_target(grandpa_justif_targets.encoded_header[i], GoldilocksField(0));
        }

        pw.set_target(grandpa_justif_targets.encoded_header_length, GoldilocksField(encoded_header.len() as u64));

        for i in 0..encoded_msg.len() {
            pw.set_target(grandpa_justif_targets.encoded_message[i], GoldilocksField(encoded_msg[i] as u64));
        }

        for i in 0..signatures.len() {
            let signature = hex::decode(signatures[i]).unwrap();

            let sig_r = decompress_point(&signature[0..32]);
            assert!(sig_r.is_valid());

            let sig_s_biguint = BigUint::from_bytes_le(&signature[32..64]);
            let sig_s = Ed25519Scalar::from_noncanonical_biguint(sig_s_biguint);
            let sig = EDDSASignature { r: sig_r, s: sig_s };

            let pubkey_bytes = hex::decode(pub_keys[i]).unwrap();
            let pub_key = decompress_point(&pubkey_bytes[..]);
            assert!(pub_key.is_valid());

            assert!(verify_message(
                &encoded_msg_bits,
                &sig,
                &EDDSAPublicKey(pub_key)
            ));

            // eddsa verification witness stuff
            pw.set_affine_point_target(&grandpa_justif_targets.pub_keys[i].0, &pub_key);
            pw.set_affine_point_target(&grandpa_justif_targets.signatures[i].r, &sig_r);
            pw.set_nonnative_target(&grandpa_justif_targets.signatures[i].s, &sig_s);
        }

        let data = builder.build::<C>();
        let proof_gen_start_time = SystemTime::now();
        let proof = data.prove(pw).unwrap();
        let proof_gen_end_time = SystemTime::now();
        let proof_gen_duration = proof_gen_end_time.duration_since(proof_gen_start_time).unwrap();

        let proof_verification_start_time = SystemTime::now();
        let verification_res = data.verify(proof);
        let proof_verification_end_time = SystemTime::now();
        let proof_verification_time = proof_verification_end_time.duration_since(proof_verification_start_time).unwrap();

        println!("proof gen time is {:?}", proof_gen_duration);
        println!("proof verification time is {:?}", proof_verification_time);

        verification_res
    }
}