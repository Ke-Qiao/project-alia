use std::fs;
fn main() {
    let directory=std::env::temp_dir().join(format!("alia-ancestor-probe-{}",std::process::id()));
    fs::create_dir_all(directory.join("new/deep")).unwrap();
    let canonical=directory.join("new/deep").canonicalize().unwrap();
    for ancestor in canonical.ancestors() {
        fs::File::open(ancestor).unwrap().sync_all().unwrap();
        println!("sync_all OK: {}", ancestor.display());
    }
    fs::remove_dir_all(directory).unwrap();
}
