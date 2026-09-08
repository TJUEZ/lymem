use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=native/kylin_vector_bridge.cpp");
    println!("cargo:rerun-if-env-changed=LYMEM_KYLIN_VECTOR_INCLUDE");
    println!("cargo:rerun-if-env-changed=LYMEM_CPP_INCLUDE");
    if env::var_os("CARGO_FEATURE_KYLIN_VECTOR").is_none() {
        return;
    }

    let include = env::var_os("LYMEM_KYLIN_VECTOR_INCLUDE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../../vendor/kylin-vector/include"));
    let required = include.join("kysdk-vector-engine-client/Database.h");
    if !required.exists() {
        panic!(
            "kylin-vector feature requires libkysdk-vector-engine-client-dev; missing {}",
            required.display()
        );
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let cpp_include = env::var_os("LYMEM_CPP_INCLUDE").map(PathBuf::from);
    let object = out.join("kylin_vector_bridge.o");
    let archive = out.join("liblymem_kylin_vector_bridge.a");
    let mut compiler = Command::new(env::var("CXX").unwrap_or_else(|_| "c++".into()));
    compiler
        .arg("-std=c++17")
        .arg("-O2")
        .arg("-fPIC")
        .arg(format!("-I{}", include.display()));
    if let Some(path) = cpp_include {
        compiler.arg(format!("-I{}", path.display()));
    }
    compiler.arg("-c").arg("native/kylin_vector_bridge.cpp").arg("-o").arg(&object);
    run(&mut compiler, "compile kylin vector bridge");
    run(
        Command::new("ar").arg("crus").arg(&archive).arg(&object),
        "archive kylin vector bridge",
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    let system_library = [
        // 容器/隔离环境:宿主根经 /proc/<引擎进程>/root 或 /run/host 可见
        "/proc/2184/root/usr/lib/x86_64-linux-gnu/libkysdk-vector-engine-client.so.1",
        "/run/host/usr/lib/x86_64-linux-gnu/libkysdk-vector-engine-client.so.1",
        // 用户级安装目录(无 root 部署)
        "/home/ez/.local/lib/kylin-vector/libkysdk-vector-engine-client.so.1",
        "/usr/lib/x86_64-linux-gnu/libkysdk-vector-engine-client.so",
        "/usr/lib/x86_64-linux-gnu/libkysdk-vector-engine-client.so.1",
        "/lib/x86_64-linux-gnu/libkysdk-vector-engine-client.so.1",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.exists())
    .unwrap_or_else(|| panic!("libkysdk-vector-engine-client runtime library is not installed"));
    let link_name = out.join("libkysdk-vector-engine-client.so");
    if !link_name.exists() {
        std::os::unix::fs::symlink(&system_library, &link_name)
            .unwrap_or_else(|e| panic!("failed to link {}: {e}", system_library.display()));
    }
    println!("cargo:rustc-link-lib=static=lymem_kylin_vector_bridge");
    println!("cargo:rustc-link-lib=dylib=kysdk-vector-engine-client");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

fn run(command: &mut Command, action: &str) {
    let status = command.status().unwrap_or_else(|e| panic!("failed to {action}: {e}"));
    if !status.success() {
        panic!("failed to {action}: {status}");
    }
}
