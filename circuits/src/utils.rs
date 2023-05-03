use std::marker::PhantomData;
use ed25519::sha512::blake2b::CHUNK_128_BYTES;

pub const NUM_VALIDATORS: usize = 10;
pub const QUORUM_SIZE: usize = 7;  // 2/3 + 1 of NUM_VALIDATORS
pub const MAX_NUM_HEADERS_PER_STEP: usize = 5;

//pub const MAX_HEADER_SIZE: usize = CHUNK_128_BYTES * 16; // 2048 bytes
pub const MAX_HEADER_SIZE: usize = CHUNK_128_BYTES * 10; // 1280 bytes.  Keep this for now.
pub const HASH_SIZE: usize = 32;                         // in bytes


use plonky2::{
    iop::{
        target::Target,
        generator::{SimpleGenerator, GeneratedValues},
        witness::{PartitionWitness, Witness}
    },
    hash::hash_types::RichField,
    plonk::circuit_builder::CircuitBuilder
};
use plonky2_field::extension::Extendable;


pub trait CircuitBuilderUtils {
    fn int_div(
        &mut self,
        dividend: Target,
        divisor: Target,
    ) -> Target;

    fn random_access_vec(
        &mut self, 
        index: Target,
        targets: Vec<Vec<Target>>,
    ) -> Vec<Target>;
}


impl<F: RichField + Extendable<D>, const D: usize> CircuitBuilderUtils for CircuitBuilder<F, D> {
    fn int_div(
        &mut self,
        dividend: Target,
        divisor: Target,
    ) -> Target {
        let quotient = self.add_virtual_target();
        let remainder = self.add_virtual_target();
    
        self.add_simple_generator(FloorDivGenerator::<F, D> {
            divisor,
            dividend,
            quotient,
            remainder,
            _marker: PhantomData
        });
        let base = self.mul(quotient, divisor);
        let rhs = self.add(base, remainder);
        let is_equal = self.is_equal(rhs, dividend);
        self.assert_one(is_equal.target);
        quotient
    }

    fn random_access_vec(
        &mut self, 
        index: Target,
        targets: Vec<Vec<Target>>,
    ) -> Vec<Target> {
        assert!(targets.len() > 0);

        let v_size = targets[0].len();

        // Assert that all vectors have the same length
        targets.iter().for_each(|t| {
            assert_eq!(t.len(), v_size);
        });

        (0..v_size).map(|i| {
            self.random_access(
                index, 
                targets.iter().map(|t| {
                    t[i]
                }).collect::<Vec<_>>())
        }).collect::<Vec<_>>()
    }

}


#[derive(Debug)]
struct FloorDivGenerator<
    F: RichField + Extendable<D>,
    const D: usize
> {
    divisor: Target,
    dividend: Target,
    quotient: Target,
    remainder: Target,
    _marker: PhantomData<F>,
}


impl<
    F: RichField + Extendable<D>,
    const D: usize,
> SimpleGenerator<F> for FloorDivGenerator<F, D> {
    fn dependencies(&self) -> Vec<Target> {
        Vec::from([self.dividend])
    }

    fn run_once(&self, witness: &PartitionWitness<F>, out_buffer: &mut GeneratedValues<F>) {
        let divisor = witness.get_target(self.divisor);
        let dividend = witness.get_target(self.dividend);
        let divisor_int = divisor.to_canonical_u64() as u32;
        let dividend_int = dividend.to_canonical_u64() as u32;
        let quotient = dividend_int / divisor_int;
        let remainder = dividend_int % divisor_int;
        out_buffer.set_target(self.quotient, F::from_canonical_u32(quotient));
        out_buffer.set_target(self.remainder, F::from_canonical_u32(remainder));
    }    
}


pub fn to_bits(msg: Vec<u8>) -> Vec<bool> {
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


// Used for testing
pub const BLOCK_576728_BLOCK_HASH: &str = "b71429ef80257a25358e386e4ca1debe72c38ea69d833e23416a4225fabb1a78";
pub const BLOCK_576728_HEADER: [u8; 1277] = [145, 37, 123, 201, 49, 223, 45, 154, 145, 243, 45, 174, 108, 166, 7, 174, 158, 65, 27, 56, 237, 135, 56, 115, 142, 175, 231, 187, 129, 109, 20, 100, 98, 51, 35, 0, 17, 7, 105, 180, 197, 184, 80, 189, 59, 130, 118, 179, 157, 175, 109, 236, 227, 36, 206, 246, 46, 33, 76, 55, 104, 167, 161, 45, 167, 168, 255, 124, 230, 60, 111, 63, 40, 141, 233, 163, 233, 202, 220, 147, 131, 92, 72, 137, 41, 229, 135, 197, 106, 156, 67, 240, 79, 42, 225, 216, 1, 144, 18, 92, 16, 6, 66, 65, 66, 69, 181, 1, 1, 0, 0, 0, 0, 242, 170, 2, 5, 0, 0, 0, 0, 132, 24, 218, 28, 195, 207, 205, 210, 155, 177, 68, 219, 195, 68, 95, 191, 78, 185, 118, 68, 23, 159, 105, 110, 197, 91, 230, 232, 78, 134, 191, 107, 168, 242, 68, 176, 161, 6, 240, 222, 175, 33, 113, 91, 182, 59, 198, 239, 156, 91, 35, 117, 88, 6, 8, 113, 180, 114, 223, 61, 248, 151, 228, 15, 219, 250, 82, 182, 184, 109, 108, 67, 40, 72, 64, 61, 19, 182, 101, 51, 156, 38, 223, 194, 83, 99, 123, 85, 63, 209, 122, 230, 61, 147, 255, 8, 4, 66, 65, 66, 69, 201, 6, 1, 40, 216, 130, 184, 127, 107, 182, 17, 4, 134, 185, 135, 25, 24, 132, 218, 176, 59, 56, 65, 133, 163, 68, 166, 208, 244, 42, 71, 152, 248, 40, 102, 126, 1, 0, 0, 0, 0, 0, 0, 0, 136, 184, 35, 131, 230, 139, 198, 231, 194, 236, 202, 246, 95, 203, 45, 254, 175, 104, 76, 11, 209, 108, 207, 30, 224, 165, 71, 31, 86, 146, 102, 18, 1, 0, 0, 0, 0, 0, 0, 0, 60, 63, 94, 197, 118, 215, 152, 192, 190, 181, 59, 63, 172, 47, 127, 56, 92, 143, 47, 142, 222, 133, 60, 222, 189, 5, 107, 142, 9, 184, 86, 43, 1, 0, 0, 0, 0, 0, 0, 0, 4, 228, 188, 183, 251, 160, 190, 148, 254, 193, 64, 107, 99, 37, 198, 184, 16, 125, 105, 157, 194, 107, 152, 89, 233, 194, 83, 247, 184, 13, 159, 111, 1, 0, 0, 0, 0, 0, 0, 0, 208, 164, 119, 102, 76, 144, 255, 230, 250, 232, 44, 62, 72, 2, 68, 28, 84, 51, 223, 186, 8, 232, 130, 158, 128, 189, 42, 115, 38, 237, 8, 9, 1, 0, 0, 0, 0, 0, 0, 0, 218, 80, 70, 177, 218, 5, 120, 187, 184, 89, 83, 46, 10, 100, 110, 58, 67, 59, 41, 197, 193, 209, 225, 93, 11, 208, 58, 209, 130, 106, 11, 42, 1, 0, 0, 0, 0, 0, 0, 0, 16, 55, 57, 204, 187, 248, 50, 52, 120, 246, 27, 83, 30, 119, 77, 120, 189, 32, 80, 46, 166, 12, 120, 128, 25, 128, 51, 197, 31, 52, 22, 77, 1, 0, 0, 0, 0, 0, 0, 0, 128, 216, 166, 193, 89, 148, 120, 136, 54, 143, 214, 96, 155, 176, 65, 100, 65, 247, 3, 144, 182, 52, 177, 187, 126, 101, 232, 253, 15, 57, 223, 58, 1, 0, 0, 0, 0, 0, 0, 0, 2, 138, 164, 210, 183, 107, 97, 45, 227, 101, 125, 94, 203, 82, 12, 224, 140, 27, 138, 166, 62, 219, 109, 150, 162, 217, 146, 83, 10, 41, 122, 56, 1, 0, 0, 0, 0, 0, 0, 0, 128, 33, 54, 130, 231, 82, 188, 228, 30, 95, 19, 157, 13, 71, 83, 165, 146, 166, 91, 247, 160, 152, 19, 19, 22, 132, 236, 176, 188, 157, 130, 57, 1, 0, 0, 0, 0, 0, 0, 0, 110, 123, 152, 217, 186, 110, 152, 80, 229, 112, 144, 152, 1, 101, 244, 202, 125, 164, 14, 61, 87, 22, 132, 66, 184, 37, 234, 255, 75, 115, 3, 11, 4, 70, 82, 78, 75, 89, 6, 1, 40, 12, 123, 33, 122, 98, 180, 207, 61, 186, 237, 4, 107, 63, 210, 223, 239, 5, 145, 32, 107, 79, 193, 173, 22, 234, 109, 207, 184, 194, 97, 76, 85, 1, 0, 0, 0, 0, 0, 0, 0, 141, 155, 21, 234, 131, 53, 39, 5, 16, 19, 91, 127, 124, 94, 249, 78, 13, 247, 14, 117, 29, 60, 95, 149, 253, 26, 166, 215, 118, 105, 41, 182, 1, 0, 0, 0, 0, 0, 0, 0, 225, 40, 141, 149, 212, 140, 18, 56, 155, 67, 152, 210, 191, 118, 153, 142, 148, 82, 196, 14, 2, 43, 214, 63, 157, 165, 41, 133, 93, 66, 123, 36, 1, 0, 0, 0, 0, 0, 0, 0, 204, 109, 230, 68, 163, 95, 75, 32, 86, 3, 250, 18, 86, 18, 223, 33, 29, 79, 157, 117, 224, 124, 132, 216, 92, 211, 94, 163, 42, 107, 28, 237, 1, 0, 0, 0, 0, 0, 0, 0, 228, 192, 138, 6, 142, 114, 164, 102, 226, 243, 119, 232, 98, 181, 178, 237, 71, 60, 79, 14, 88, 215, 210, 101, 161, 35, 173, 17, 254, 242, 167, 151, 1, 0, 0, 0, 0, 0, 0, 0, 43, 167, 192, 11, 252, 193, 43, 86, 163, 6, 196, 30, 196, 76, 65, 16, 66, 208, 184, 55, 164, 13, 128, 252, 101, 47, 165, 140, 207, 183, 134, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7, 149, 144, 223, 52, 205, 31, 162, 248, 60, 177, 239, 119, 11, 62, 37, 74, 187, 0, 250, 125, 191, 178, 247, 242, 27, 56, 58, 122, 114, 107, 178, 1, 0, 0, 0, 0, 0, 0, 0, 51, 90, 68, 109, 85, 107, 216, 177, 45, 46, 135, 178, 194, 176, 162, 182, 18, 248, 156, 149, 154, 198, 15, 149, 92, 51, 68, 137, 192, 54, 62, 67, 1, 0, 0, 0, 0, 0, 0, 0, 212, 187, 136, 245, 207, 81, 198, 76, 152, 253, 220, 241, 56, 57, 164, 141, 227, 88, 89, 128, 78, 78, 59, 109, 178, 39, 233, 177, 87, 216, 50, 236, 1, 0, 0, 0, 0, 0, 0, 0, 72, 62, 116, 144, 188, 18, 164, 231, 130, 34, 74, 81, 59, 191, 88, 29, 253, 133, 232, 145, 23, 180, 224, 245, 102, 59, 119, 7, 94, 4, 16, 151, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 66, 65, 66, 69, 1, 1, 68, 38, 184, 111, 57, 251, 224, 57, 76, 122, 252, 3, 59, 213, 125, 237, 203, 217, 95, 110, 161, 172, 125, 243, 205, 231, 220, 141, 176, 144, 126, 20, 109, 214, 126, 193, 204, 47, 243, 254, 84, 203, 155, 185, 169, 244, 136, 224, 72, 157, 110, 212, 194, 170, 111, 181, 227, 52, 184, 92, 208, 199, 56, 141, 0, 4, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 129, 1, 178, 84, 15, 97, 151, 104, 188, 59, 189, 239, 81, 107, 33, 200, 101, 245, 169, 243, 185, 20, 93, 71, 169, 146, 85, 96, 42, 98, 209, 39, 111, 83, 100, 63, 153, 43, 69, 194, 55, 129, 127, 71, 16, 205, 1, 65, 47, 74, 178, 84, 15, 97, 151, 104, 188, 59, 189, 239, 81, 107, 33, 200, 101, 245, 169, 243, 185, 20, 93, 71, 169, 146, 85, 96, 42, 98, 209, 39, 111, 83, 100, 63, 153, 43, 69, 194, 55, 129, 127, 71, 16, 205, 1, 65, 47, 74, 4, 0];
pub const BLOCK_576728_PARENT_HASH: &str = "91257bc931df2d9a91f32dae6ca607ae9e411b38ed8738738eafe7bb816d1464";
pub const BLOCK_576728_STATE_ROOT: &str = "110769b4c5b850bd3b8276b39daf6dece324cef62e214c3768a7a12da7a8ff7c";