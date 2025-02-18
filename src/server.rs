use actix_web::{web, App, HttpServer, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[derive(Debug, Serialize, Deserialize)]
struct RustSourceRequest {
    source_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct MirResponse {
    success: bool,
    content: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListFilesRequest {
    dir_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListFilesResponse {
    filenames: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PnAnalysisRequest {
    source_code: String,
    mode: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PnAnalysisResponse {
    graph_content: String,
    output: String,
    error: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileContentRequest {
    file_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileContentResponse {
    content: String,
}

async fn get_mir(rust_source: web::Json<RustSourceRequest>) -> impl Responder {
    let source_code = rust_source.source_code.clone();

    // Create a temporary directory
    let temp_dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to create temp dir: {}", e)),
    };

    // Create a temporary source file
    let source_path = temp_dir.path().join("main.rs");
    if let Err(e) = File::create(&source_path).and_then(|mut file| file.write_all(source_code.as_bytes())) {
        return HttpResponse::InternalServerError().json(format!("Failed to write source file: {}", e));
    }

    // Get rustc path
    let rustc_path = match which::which("rustc") {
        Ok(path) => path,
        Err(e) => return HttpResponse::InternalServerError().json(format!("rustc not found: {}", e)),
    };

    // Execute rustc command to get MIR
    let output = match Command::new(rustc_path)
        .arg("--crate-name=rust_mir")
        .arg("-Zunpretty=mir")
        .arg(&source_path)
        .output() {
        Ok(output) => output,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to execute rustc: {}", e)),
    };

    // Construct response
    let response = MirResponse {
        success: output.status.success(),
        content: if output.status.success() {
            String::from_utf8_lossy(&output.stdout).into_owned()
        } else {
            String::from_utf8_lossy(&output.stderr).into_owned()
        },
    };

    HttpResponse::Ok().json(response)
}

async fn list_files(req: web::Json<ListFilesRequest>) -> impl Responder {
    let dir_path = &req.dir_path;
    let mut filenames = Vec::new();

    match fs::read_dir(dir_path) {
        Ok(entries) => {
            for entry in entries {
                if let Ok(entry) = entry {
                    if let Some(name) = entry.file_name().to_str() {
                        filenames.push(name.to_string());
                    }
                }
            }
            HttpResponse::Ok().json(ListFilesResponse { filenames })
        },
        Err(e) => HttpResponse::InternalServerError().json(format!("Failed to read directory {}: {}", dir_path, e)),
    }
}

async fn save_source_code(source_code: String) -> Result<PathBuf, String> {
    let dir_path = PathBuf::from("/home/kevin/tmpfile");
    fs::create_dir_all(&dir_path)
        .map_err(|e| format!("Failed to create directory {}: {}", dir_path.display(), e))?;
    let file_path = dir_path.join("main.rs");
    fs::write(&file_path, source_code)
        .map_err(|e| format!("Failed to write source code: {}", e))?;
    Ok(dir_path)
}

async fn run_pn_analysis(req: web::Json<PnAnalysisRequest>) -> impl Responder {
    let source_code = req.source_code.clone();
    let mode = req.mode.clone();

    // 1. Save source code to temporary directory
    let tmp_dir = match save_source_code(source_code).await {
        Ok(dir) => dir,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to save source code: {}", e)),
    };
    let source_file = tmp_dir.join("main.rs");
    let graph_dot = tmp_dir.join("tmp").join("main").join("graph.dot");

    // 2. Construct pn command arguments
    let pn_flags = format!(
        "{} -p main --pn-analysis-dir={}/tmp/ --viz-petrinet",
        mode,
        tmp_dir.to_string_lossy()
    );

    // 3. Execute pn command
    let pn_path = match which::which("pn") {
        Ok(path) => path,
        Err(e) => return HttpResponse::InternalServerError().json(format!("pn not found: {}", e)),
    };
    let output = match Command::new(pn_path)
        .env("PN_FLAGS", pn_flags)
        .arg(&source_file)
        .output() {
        Ok(output) => output,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to execute pn: {}", e)),
    };

    // 4. Execute dot command to convert dot file to SVG
    let dot_path = match which::which("dot") {
        Ok(path) => path,
        Err(e) => return HttpResponse::InternalServerError().json(format!("dot not found: {}", e)),
    };
    let mut dot_process = match Command::new(dot_path)
        .args(["-Tsvg"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn() {
        Ok(process) => process,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to execute dot: {}", e)),
    };

    // 5. Read generated dot file content
    let dot_content = match fs::read_to_string(&graph_dot) {
        Ok(content) => content,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to read dot file: {}", e)),
    };

    // 6. Write dot content to dot command's standard input
    if let Some(mut stdin) = dot_process.stdin.take() {
        if let Err(e) = stdin.write_all(dot_content.as_bytes()) {
            return HttpResponse::InternalServerError().json(format!("Failed to write to dot stdin: {}", e));
        }
    }

    // 7. Get dot command's output (SVG content)
    let output_svg = match dot_process.wait_with_output() {
        Ok(output) => output,
        Err(e) => return HttpResponse::InternalServerError().json(format!("Failed to wait for dot process: {}", e)),
    };
    let svg_content = String::from_utf8_lossy(&output_svg.stdout).to_string();

    // 8. Construct response
    let response = PnAnalysisResponse {
        graph_content: svg_content,
        output: String::from_utf8_lossy(&output.stdout).to_string(),
        error: String::from_utf8_lossy(&output.stderr).to_string(),
    };

    HttpResponse::Ok().json(response)
}

async fn get_file_content(req: web::Json<FileContentRequest>) -> impl Responder {
    let file_path = &req.file_path;

    match fs::read_to_string(file_path) {
        Ok(content) => HttpResponse::Ok().json(FileContentResponse { content }),
        Err(e) => HttpResponse::InternalServerError().json(format!("Failed to read file {}: {}", file_path, e)),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("RustMir Server listening on 0.0.0.0:8080");

    HttpServer::new(|| {
        App::new()
            .route("/get_mir", web::post().to(get_mir))
            .route("/list_files", web::post().to(list_files))
            .route("/run_pn_analysis", web::post().to(run_pn_analysis))
            .route("/get_file_content", web::post().to(get_file_content))
    })
        .bind("0.0.0.0:8080")?
        .run()
        .await
}