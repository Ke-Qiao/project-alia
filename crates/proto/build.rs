fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                "proto/process.proto",
                "proto/audio.proto",
                "proto/binding.proto",
                "proto/plugin.proto",
                "proto/settings.proto",
            ],
            &["proto/"],
        )?;
    Ok(())
}
