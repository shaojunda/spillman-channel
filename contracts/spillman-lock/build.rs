use ckb_gen_types::{packed::CellOutput, prelude::*};
use std::env;
use std::fs::{read, File};
use std::io::{BufWriter, Write};
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../deps/auth");

    // Generate AUTH_CODE_HASH
    let auth_binary = read("../../deps/auth").expect("read auth binary");
    let code_hash = CellOutput::calc_data_hash(&auth_binary);

    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("auth_code_hash.rs");
    let mut out_file = BufWriter::new(File::create(out_path).expect("create auth_code_hash.rs"));

    writeln!(
        &mut out_file,
        "pub const AUTH_CODE_HASH: [u8; 32] = {:#02X?};",
        code_hash.as_slice()
    )
    .expect("write to auth_code_hash.rs");

    // Generate SECP256K1_CODE_HASH
    let secp256k1_code_hash = hex::decode(
        "9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8"
    ).expect("decode secp256k1 code hash");

    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("secp256k1_code_hash.rs");
    let mut out_file = BufWriter::new(File::create(out_path).expect("create secp256k1_code_hash.rs"));

    write!(
        &mut out_file,
        "pub const SECP256K1_CODE_HASH: [u8; 32] = ["
    )
    .expect("write to secp256k1_code_hash.rs");

    for (i, byte) in secp256k1_code_hash.iter().enumerate() {
        if i > 0 {
            write!(&mut out_file, ", ").expect("write comma");
        }
        write!(&mut out_file, "{:#02X}", byte).expect("write byte");
    }

    writeln!(&mut out_file, "];").expect("write closing bracket");
}
