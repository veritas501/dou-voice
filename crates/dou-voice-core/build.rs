fn main() {
    println!("cargo:rerun-if-changed=proto/pbws.proto");
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("find vendored protoc");
    std::env::set_var("PROTOC", protoc);

    prost_build::compile_protos(&["proto/pbws.proto"], &["proto"]).expect("compile pbws proto");
}
