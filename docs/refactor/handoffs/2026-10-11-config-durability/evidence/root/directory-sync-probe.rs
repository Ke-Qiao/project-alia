fn main() {
    let directory = std::env::temp_dir().join(format!("alia-dir-sync-probe-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let file = std::fs::File::open(&directory).unwrap();
    let result = file.sync_all();
    println!("std directory sync_all: {result:?}");
    std::fs::remove_dir(&directory).unwrap();
}
