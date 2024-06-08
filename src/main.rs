// Synchronize directory

use std::str::FromStr;
use clap::Parser;

use futures_util::{stream, SinkExt, StreamExt};

use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use tokio::net::{TcpListener, TcpStream};

use zip::ZipArchive;

use std::collections::VecDeque;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::env;
use zip::{ZipWriter, CompressionMethod, write::FileOptions};

#[derive(Parser, Debug)]
#[command(about = "Send a directory using websockets", long_about = None)]
struct Args {
    #[arg(long)]
    port: Option<usize>,
    #[arg(long = "output-dir")]
    output_dir: Option<std::path::PathBuf>,

    #[arg(long)]
    from: Option<std::path::PathBuf>,
    #[arg(long)]
    to: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let result = match (args.port, args.output_dir, args.from, args.to) {
        (Some(port), Some(output_dir), None, None) => sd_receiver_init(port, output_dir).await,
        (None, None, Some(from), Some(to)) => sd_sender_init(from, to).await,
        _ => panic!("Invalid arguments"),
    };

    match result {
        Ok(()) => {},
        Err(e) => println!("ERROR: {}", e),
    }
}

///////////////////////////////////////////////////////////
// RECEIVER
///////////////////////////////////////////////////////////

async fn sd_receiver_init(port: usize, output_dir: std::path::PathBuf) -> Result<(), Box<dyn Error>> {
    let url = format!("ws://localhost:{}", port);
    println!("Syncing directory sent from {} to {:?}", url, output_dir);

    // Remove output directory if it exists
    if output_dir.exists() {
        println!("Removing existing output directory {:?}", output_dir);
        fs::remove_dir_all(&output_dir)?;
    }

    // Create output directory
    fs::create_dir_all(&output_dir)?;
    let zip_output = format!("{}/compressed_files.zip", output_dir.as_path().to_str().expect("Invalid output directory"));
    let zip_file = std::path::PathBuf::from_str(zip_output.as_str())?;
    std::fs::write(&zip_file, "")?;

    // Create WebSocket connection
    let (ws_stream, _) = connect_async(&url).await?;
    println!("Created Websocket connection with {}", url);
    let (_, read) = ws_stream.split();

    // Read data from WebSocket connection and write to ZIP file
    println!("Writing data to ZIP file");
    let ws_to_stdout = {
        read.for_each(|message| async {
            let message = message.expect("Invalid message received");
            if message.is_close() {
                println!("Close WebSocket message received");
                return;
            }
            let data = message.into_data();
            let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&zip_output).expect("Unable to open ZIP file");

            file.write_all(&data).expect("Error appending to ZIP file");
        })
    };
    ws_to_stdout.await;

    // Extract ZIP file
    println!("Extracting ZIP file");
    extract_zip(&zip_file, &output_dir)?;
    std::fs::remove_file(zip_file)?;

    println!("Successfully synced directory to {:?}", output_dir);
    Ok(())
}

fn extract_zip(zip_file_path: &std::path::PathBuf, output_dir: &std::path::PathBuf) -> Result<(), Box<dyn Error>> {
    let mut archive = ZipArchive::new(File::open(zip_file_path)?)?;

    // Iterate through the files in the ZIP archive.
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;

        // Create the path to the extracted file in the destination directory.
        let target_path = output_dir.join(file.name());

        // Create the destination directory if it does not exist.
        if let Some(parent_dir) = target_path.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }

        let mut output_file = File::create(&target_path)?;

        // Read the contents of the file from the ZIP archive and write them to the destination file.
        io::copy(&mut file, &mut output_file)?;
    }

    Ok(())
}

///////////////////////////////////////////////////////////
// SENDER
///////////////////////////////////////////////////////////

async fn sd_sender_init(from: std::path::PathBuf, to: String) -> Result<(), Box<dyn Error>> {
    // Create ZIP file
    let curr_dir = env::current_dir()?;
    let dir_path = format!("{}/compressed_files.zip", curr_dir.as_path().to_str().expect("Invalid current directory"));
    let zip_file_path = Path::new(&dir_path);
    println!("Creating ZIP file");
    create_zip(&zip_file_path, &from)?;
    println!("Finished creating ZIP file");

    let addr = &to[5..];
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Ready to sync directory. Listening on: {}", addr);
    accept_connection(listener.accept().await?.0).await?;
    std::fs::remove_file(zip_file_path)?;
    println!("Successfully sent directory {:?}", from);
    Ok(())
}

fn create_zip(zip_file_path: &Path, from: &std::path::PathBuf) -> Result<(), Box<dyn Error>> {
    let zip_file = File::create(&zip_file_path)?;

    let mut zip = ZipWriter::new(zip_file);
    let mut dirs = VecDeque::from(vec![from.clone()]);

    // Set file options (e.g., compression method)
    let options = FileOptions::default()
        .compression_method(CompressionMethod::DEFLATE);

    // Iterate through the files and add them to the ZIP archive.
    loop {
        let dir = match dirs.pop_front() {
            Some(dir) => dir,
            None => break,
        };
        for file in fs::read_dir(dir)? {
            let file_path = file?.path();

            if file_path.is_dir() {
                dirs.push_back(file_path);
                continue
            }

            // Adding the file to the ZIP archive.
            let file_name_temp = file_path.to_str().expect("Invalid ZIP file path").strip_prefix(from.as_path().to_str().expect("Invalid from path")).expect("Unable to strip prefix from absolute file path");
            let file_name = file_name_temp.strip_prefix("/").unwrap_or(file_name_temp);
            zip.start_file(file_name, options)?;

            let mut buffer = Vec::new();
            let file = File::open(file_path)?;
            io::copy(&mut file.take(u64::MAX), &mut buffer)?;

            zip.write_all(&buffer)?;
        }
    }

    zip.finish()?;

    Ok(())
}

async fn accept_connection(stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let addr = stream.peer_addr().expect("connected streams should have a peer address");
    println!("New WebSocket connection: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .expect("Error during the websocket handshake occurred");


    let (mut write, _) = ws_stream.split();
    let curr_dir = env::current_dir()?;
    let dir_path = format!("{}/compressed_files.zip", curr_dir.as_path().to_str().expect("Invalid current directory"));
    let f = File::open(dir_path)?;

    let reader = BufReader::new(f);
    let mut stream = stream::iter(reader.bytes()).map(|byte| Ok(Message::binary(byte?.to_be_bytes())));
    write.send_all(&mut stream).await?;
    println!("Closing WebSocket connection");
    write.close().await?;
    
    Ok(())
}