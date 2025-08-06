use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub(crate) fn init_log_file(
    folder_name: &str,
    log_file_name: &str,
) -> Result<File, Box<dyn std::error::Error>> {
    fs::create_dir_all(folder_name)?;

    let log_path = Path::new(folder_name).join(log_file_name);

    let mut log_file = fs::File::create(&log_path)?;

    writeln!(
        log_file,
        "[+] Log file initialized",
    )?;
    writeln!(
        log_file,
        "[+] Application started",
    )?;

    println!("Log setup complete: {}", log_path.display());
    Ok(log_file)
}

pub(crate) fn setup_hlocal_logging(
    clusters: usize,
) -> Result<Vec<File>, Box<dyn std::error::Error>> {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d_%H:%M:%S");
    let folder_name = format!("logs/{timestamp}");

    let mut files = vec![];

    for i in 0..clusters {
        let log_file_name = format!("cluster{i}.log");
        files.push(init_log_file(&folder_name, &log_file_name)?);
    }

    files.push(init_log_file(&folder_name, "GVT.log")?);

    Ok(files)
}
