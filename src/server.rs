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
use rustmir::{
    ListFilesRequest, ListFilesResponse, PnAnalysisRequest, PnAnalysisResponse, RustSourceRequest,
    MirResponse, FileContentRequest, FileContentResponse
};

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

/// 保存传入的源代码到临时目录，并返回该目录路径
async fn save_source_code(source_code: String) -> Result<PathBuf, String> {
    let dir_path = PathBuf::from("/home/kevin/tmpfile");
    fs::create_dir_all(&dir_path)
        .map_err(|e| format!("Failed to create directory {}: {}", dir_path.display(), e))?;
    let file_path = dir_path.join("main.rs");
    fs::write(&file_path, source_code)
        .map_err(|e| format!("Failed to write source code: {}", e))?;
    Ok(dir_path)
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

    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<ListFilesResponse>, Status> {
        let req = request.into_inner();
        let dir_path = req.dir_path;
        let mut filenames = Vec::new();
        let entries = fs::read_dir(&dir_path)
            .map_err(|e| Status::internal(format!("Failed to read directory {}: {}", dir_path, e)))?;
        for entry in entries {
            let entry = entry
                .map_err(|e| Status::internal(format!("Error reading entry: {}", e)))?;
            if let Some(name) = entry.file_name().to_str() {
                filenames.push(name.to_string());
            }
        }
        let response = ListFilesResponse { filenames };
        Ok(Response::new(response))
    }

    async fn run_pn_analysis(
        &self,
        request: Request<PnAnalysisRequest>,
    ) -> Result<Response<PnAnalysisResponse>, Status> {
        let req = request.into_inner();
        let source_code = req.source_code;
        let mode = req.mode;

        // 1. 保存源代码到临时目录
        let tmp_dir = save_source_code(source_code)
            .await
            .map_err(|e| Status::internal(e))?;
        let source_file = tmp_dir.join("main.rs");
        // 根据你的 pn 工具生成 dot 文件的位置调整路径
        let graph_dot = tmp_dir.join("tmp").join("main").join("graph.dot");

        // 2. 构造 pn 命令参数
        let pn_flags = format!(
            "{} -p main --pn-analysis-dir={}/tmp/ --viz-petrinet",
            mode,
            tmp_dir.to_string_lossy()
        );

        // 3. 执行 pn 命令
        let pn_path = Self::get_command_path("pn").map_err(|e| Status::internal(e))?;
        let output = Command::new(pn_path)
            .env("PN_FLAGS", pn_flags)
            .arg(&source_file)
            .output()
            .map_err(|e| Status::internal(format!("Failed to execute pn: {}", e)))?;

        // 4. 执行 dot 命令，将 dot 文件转换为 SVG
        let dot_path = Self::get_command_path("dot").map_err(|e| Status::internal(e))?;
        let mut dot_process = Command::new(dot_path)
            .args(["-Tsvg"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Status::internal(format!("Failed to execute dot: {}", e)))?;

        // 5. 读取生成的 dot 文件内容
        let dot_content = fs::read_to_string(&graph_dot)
            .map_err(|e| Status::internal(format!("Failed to read dot file: {}", e)))?;

        // 6. 将 dot 内容写入 dot 命令的标准输入
        if let Some(mut stdin) = dot_process.stdin.take() {
            stdin
                .write_all(dot_content.as_bytes())
                .map_err(|e| Status::internal(format!("Failed to write to dot stdin: {}", e)))?;
        }

        // 7. 获取 dot 命令的输出（SVG 内容）
        let output_svg = dot_process
            .wait_with_output()
            .map_err(|e| Status::internal(format!("Failed to wait for dot process: {}", e)))?;
        let svg_content = String::from_utf8_lossy(&output_svg.stdout).to_string();

        // 8. 构造返回结果
        let response = PnAnalysisResponse {
            graph_content: svg_content,
            output: String::from_utf8_lossy(&output.stdout).to_string(),
            error: String::from_utf8_lossy(&output.stderr).to_string(),
        };

        Ok(Response::new(response))
    }

    async fn get_file_content(
        &self,
        request: Request<FileContentRequest>,
    ) -> Result<Response<FileContentResponse>, Status> {
        // 从请求中获取文件路径
        let req = request.into_inner();
        let file_path = req.file_path;

        // 读取文件内容
        let content = fs::read_to_string(&file_path)
            .map_err(|e| Status::internal(format!("Failed to read file {}: {}", file_path, e)))?;

        // 构造响应
        let response = FileContentResponse { content };

        Ok(Response::new(response))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;
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