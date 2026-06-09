fn main() -> Result<(), Box<dyn std::error::Error>> {
    // protoc-bin-vendored 暴露平台对应的预编译 protoc 二进制；
    // tonic-build 通过环境变量 PROTOC 找到它，无需用户本地安装。
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/control.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/control.proto");
    Ok(())
}
