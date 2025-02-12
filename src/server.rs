use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;
use tonic::{transport::Server, Request, Response, Status};

pub mod rustmir {
    include!(concat!(env!("OUT_DIR"), "/rustmir.rs"));
}

use rustmir::rust_mir_service_server::{RustMirService, RustMirServiceServer};
use rustmir::{RustSourceRequest, MirResponse};

#[derive(Debug, Default)]
pub struct MyRustMirService {}

impl MyRustMirService {
    // 获取命令路径
    fn get_command_path(command: &str) -> Result<String, String> {
        which::which(command)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|e| format!("Command {} not found: {}", command, e))
    }
}

#[tonic::async_trait]
impl RustMirService for MyRustMirService {
    async fn get_mir(
        &self,
        request: Request<RustSourceRequest>,
    ) -> Result<Response<MirResponse>, Status> {
        let source_code = request.into_inner().source_code;

        // 创建临时目录
        let temp_dir = TempDir::new()
            .map_err(|e| Status::internal(format!("Failed to create temp dir: {}", e)))?;

        // 创建临时源文件
        let source_path = temp_dir.path().join("main.rs");
        File::create(&source_path)
            .and_then(|mut file| file.write_all(source_code.as_bytes()))
            .map_err(|e| Status::internal(format!("Failed to write source file: {}", e)))?;

        // 获取 rustc 路径
        let rustc_path = Self::get_command_path("rustc")
            .map_err(|e| Status::internal(e))?;

        // 执行 rustc 命令获取 MIR
        let output = Command::new(rustc_path)
            .arg("--crate-name=rust_mir")
            .arg("-Zunpretty=mir")
            .arg(&source_path)
            .output()
            .map_err(|e| Status::internal(format!("Failed to execute rustc: {}", e)))?;

        // 构造响应
        let response = MirResponse {
            success: output.status.success(),
            content: if output.status.success() {
                String::from_utf8_lossy(&output.stdout).into_owned()
            } else {
                String::from_utf8_lossy(&output.stderr).into_owned()
            },
        };

        Ok(Response::new(response))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse()?;
    let mir_service = MyRustMirService::default();

    println!("RustMir Server listening on {}", addr);

    Server::builder()
        .add_service(RustMirServiceServer::new(mir_service))
        .serve(addr)
        .await?;

    Ok(())
}

// let request = RustSourceRequest {
//     source_code: r#"
//         fn main() {
//             println!("Hello, World!");
//         }
//     "#.to_string(),
// };