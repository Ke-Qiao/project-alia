use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use alia_server::openapi::ApiDoc;
use utoipa::OpenApi;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: openapi <output.json>");
        return ExitCode::from(2);
    };
    if args.next().is_some() || path.is_empty() {
        eprintln!("usage: openapi <output.json>");
        return ExitCode::from(2);
    }

    match write_spec(Path::new(&path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("openapi: {error}");
            ExitCode::from(1)
        }
    }
}

fn write_spec(output: &Path) -> io::Result<()> {
    let file_name = output.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "output path has no file name")
    })?;
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_vec_pretty(&ApiDoc::openapi()).map_err(io::Error::other)?;
    let temp = output.with_file_name(format!(".{}.tmp", file_name.to_string_lossy()));
    let write_result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&json)?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, output) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}
