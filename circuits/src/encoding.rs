use plonky2::hash::hash_types::RichField;
use plonky2::iop::target::Target;
use plonky2::plonk::circuit_builder::CircuitBuilder;
use plonky2_field::extension::Extendable;
use crate::utils::{ CircuitBuilderUtils, HASH_SIZE, MAX_HEADER_SIZE };

trait CircuitBuilderScaleDecoder {
    fn decode_compact_int(
        &mut self,
        compact_bytes: Vec<Target>,
    ) -> (Target, Target, Target);
}

// This assumes that all the inputted byte array are already range checked (e.g. all bytes are less than 256)
impl<F: RichField + Extendable<D>, const D: usize> CircuitBuilderScaleDecoder for CircuitBuilder<F, D> {
    fn decode_compact_int(
        &mut self,
        compact_bytes: Vec<Target>
    ) -> (Target, Target, Target) {
        // For now, assume that compact_bytes is 5 bytes long
        assert!(compact_bytes.len() == 5);

        let bits = self.split_le(compact_bytes[0], 8);
        let compress_mode = self.le_sum(bits[0..2].iter());

        // Get all of the possible bytes that could be used to represent the compact int

        let zero_mode_value = compact_bytes[0];
        let one_mode_value = self.reduce(256, compact_bytes[0..2].to_vec());
        let two_mode_value = self.reduce(256, compact_bytes[0..4].to_vec());
        let three_mode_value = self.reduce(256, compact_bytes[1..5].to_vec());
        let value = self.random_access(compress_mode, vec![zero_mode_value, one_mode_value, two_mode_value, three_mode_value]);

        // Will need to divide by 4 (remove least 2 significnat bits) for mode 0, 1, 2.  Those bits stores the encoding mode
        let three = self.constant(F::from_canonical_u8(3));
        let is_eq_three = self.is_equal(compress_mode, three);
        let div_by_4 = self.not(is_eq_three);

        let four = self.constant(F::from_canonical_u8(4));
        let value_div_4 = self.int_div(value, four);

        let decoded_int = self.select(div_by_4, value_div_4, value);

        let five = self.constant(F::from_canonical_u8(5));
        let one = self.one();
        let two = self.two();
        let encoded_byte_length = self.random_access(compress_mode, vec![one, two, four, five]);

        (decoded_int, compress_mode, encoded_byte_length)
    }
}


struct EncodedHeaderTarget {
    header_bytes: Vec<Target>,
    header_size: Target,
}

struct HeaderTarget {
    block_number: Target,
    parent_hash: Vec<Target>,    // Vector of 32 bytes
    state_root: Vec<Target>,     // Vector of 32 bytes
    //data_root: Vec<Target>,      // Vector of 32 bytes
}


trait CircuitBuilderHeaderDecoder {
    fn decode_header(
        &mut self,
        header: EncodedHeaderTarget,
    ) -> HeaderTarget;
}

// This assumes that all the inputted byte array are already range checked (e.g. all bytes are less than 256)
impl<F: RichField + Extendable<D>, const D: usize> CircuitBuilderHeaderDecoder for CircuitBuilder<F, D> {
    fn decode_header(
        &mut self,
        header: EncodedHeaderTarget,
    ) -> HeaderTarget {

        // The first 32 bytes are the parent hash
        let parent_hash_target = header.header_bytes[0..32].to_vec();

        // Next field is the block number
        // Can need up to 5 bytes to represent a compact u32
        const MAX_BLOCK_NUMBER_SIZE: usize = 5;
        let (block_number_target, compress_mode, _) = self.decode_compact_int(header.header_bytes[32..32+MAX_BLOCK_NUMBER_SIZE].to_vec());

        let mut all_possible_state_roots = Vec::new();
        all_possible_state_roots.push(header.header_bytes[33..33+HASH_SIZE].to_vec());
        all_possible_state_roots.push(header.header_bytes[34..34+HASH_SIZE].to_vec());
        all_possible_state_roots.push(header.header_bytes[36..36+HASH_SIZE].to_vec());
        all_possible_state_roots.push(header.header_bytes[37..37+HASH_SIZE].to_vec());

        let state_root_target = self.random_access_vec(compress_mode, all_possible_state_roots);

        /*
        let mut all_possible_data_roots = Vec::new();

        // 98 is the minimum total size of all the header's fields before the data root
        const DATA_ROOT_MIN_START_IDX: usize = 98;
        for start_idx in DATA_ROOT_MIN_START_IDX..MAX_HEADER_SIZE - HASH_SIZE {
            all_possible_data_roots.push(header.header_bytes[start_idx..start_idx+HASH_SIZE].to_vec());
        }

        // Need to pad all_possible_data_roots to be length of a power of 2
        let min_power_of_2 = ((MAX_HEADER_SIZE - HASH_SIZE) as f32).log2().ceil() as usize;
        let all_possible_data_roots_size = 2usize.pow(min_power_of_2 as u32);
        for _ in all_possible_data_roots.len()..all_possible_data_roots_size {
            all_possible_data_roots.push(vec![self.zero(); HASH_SIZE]);
        }

        let ninety_eight = self.constant(F::from_canonical_usize(DATA_ROOT_MIN_START_IDX));
        let data_root_idx = self.sub(header.header_size, ninety_eight);
        let data_root_target = self.random_access_vec(data_root_idx, all_possible_data_roots);
        */

        HeaderTarget {
            parent_hash: parent_hash_target,
            block_number: block_number_target,
            state_root: state_root_target,
            //data_root: data_root_target,
        }
    }
}


#[cfg(test)]
mod tests {
    use anyhow::Result;
    use plonky2::iop::witness::{PartialWitness, Witness};
    use plonky2::plonk::circuit_builder::CircuitBuilder;
    use plonky2::plonk::circuit_data::CircuitConfig;
    use plonky2::plonk::config::{GenericConfig, PoseidonGoldilocksConfig};
    use plonky2_field::types::Field;

    use crate::utils::{BLOCK_576728_HEADER, BLOCK_576728_PARENT_HASH, BLOCK_576728_STATE_ROOT, MAX_HEADER_SIZE, HASH_SIZE};
    use crate::encoding::{ CircuitBuilderScaleDecoder, CircuitBuilderHeaderDecoder, EncodedHeaderTarget };


    fn test_compact_int(
        encoded_bytes: [u8; 5],
        expected_int: u64,
        expected_compress_mode: u8,
        expected_length: u8
    ) -> Result<()> {
        const D: usize = 2;
        type C = PoseidonGoldilocksConfig;
        type F = <C as GenericConfig<D>>::F;
        let config = CircuitConfig::standard_recursion_config();
        let pw = PartialWitness::new();
        let mut builder = CircuitBuilder::<F, D>::new(config);

        let mut encoded_bytes_target = Vec::new();

        for i in 0..encoded_bytes.len() {
            encoded_bytes_target.push(builder.constant(F::from_canonical_u8(encoded_bytes[i])));
        }

        let (decoded_int, compress_mode, length) = builder.decode_compact_int(encoded_bytes_target);

        let expected_int = builder.constant(F::from_canonical_u64(expected_int));
        builder.connect(decoded_int, expected_int);

        let expected_compress_mode = builder.constant(F::from_canonical_u8(expected_compress_mode));
        builder.connect(compress_mode, expected_compress_mode);

        let expected_length = builder.constant(F::from_canonical_u8(expected_length));
        builder.connect(length, expected_length);
        
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;

        data.verify(proof)
    }

    #[test]
    fn test_decode_compact_int_0() -> Result<()> {
        let encoded_bytes = [0u8; 5];
        let expected_value = 0;
        test_compact_int(encoded_bytes, expected_value, 0, 1)
    }

    #[test]
    fn test_decode_compact_int_1() -> Result<()> {
        let encoded_bytes = [4, 0, 0, 0, 0];
        let expected_value = 1;
        test_compact_int(encoded_bytes, expected_value, 0, 1)
    }

    #[test]
    fn test_decode_compact_int_64() -> Result<()> {
        let encoded_bytes = [1, 1, 0, 0, 0];
        let expected_value = 64;
        test_compact_int(encoded_bytes, expected_value, 1, 2)
    }

    #[test]
    fn test_decode_compact_int_65() -> Result<()> {
        let encoded_bytes = [5, 1, 0, 0, 0];
        let expected_value = 65;
        test_compact_int(encoded_bytes, expected_value, 1, 2)
    }

    #[test]
    fn test_decode_compact_int_16384() -> Result<()>  {
        let encoded_bytes = [2, 0, 1, 0, 0];
        let expected_value = 16384;
        test_compact_int(encoded_bytes, expected_value, 2, 4)
    }

    #[test]
    fn test_decode_compact_int_1073741824() -> Result<()> {
        let encoded_bytes = [3, 0, 0, 0, 64];
        let expected_value = 1073741824;
        test_compact_int(encoded_bytes, expected_value, 3, 5)
    }

    #[test]
    fn test_decode_block() -> Result<()> {
        const D: usize = 2;
        type C = PoseidonGoldilocksConfig;
        type F = <C as GenericConfig<D>>::F;
        let config = CircuitConfig::standard_recursion_config();
        let pw = PartialWitness::new();
        let mut builder = CircuitBuilder::<F, D>::new(config);

        let mut header_bytes_target = BLOCK_576728_HEADER.iter().map(|b| {
            builder.constant(F::from_canonical_u8(*b))
        }).collect::<Vec<_>>();
        let header_size = builder.constant(F::from_canonical_usize(BLOCK_576728_HEADER.len()));

        // pad the header bytes
        for _ in BLOCK_576728_HEADER.len()..MAX_HEADER_SIZE {
            header_bytes_target.push(builder.zero());
        }

        let decoded_header = builder.decode_header(EncodedHeaderTarget{header_bytes: header_bytes_target, header_size});

        let expected_block_number = builder.constant(F::from_canonical_u64(576728));
        builder.connect(decoded_header.block_number, expected_block_number);

        let expected_parent_hash = hex::decode(BLOCK_576728_PARENT_HASH).unwrap();
        for i in 0..expected_parent_hash.len() {
            let expected_parent_hash_byte = builder.constant(F::from_canonical_u8(expected_parent_hash[i]));
            builder.connect(decoded_header.parent_hash[i], expected_parent_hash_byte);
        }

        let expected_state_root = hex::decode(BLOCK_576728_STATE_ROOT).unwrap();
        for i in 0..expected_state_root.len() {
            let expected_state_root_byte = builder.constant(F::from_canonical_u8(expected_state_root[i]));
            builder.connect(decoded_header.state_root[i], expected_state_root_byte);
        }

        let data = builder.build::<C>();
        let proof = data.prove(pw)?;

        data.verify(proof)
    }
}
