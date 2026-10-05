pub mod voice {
    tonic::include_proto!("alia.voice.v2");
}

pub mod playback {
    tonic::include_proto!("alia.playback.v3");
}

pub mod output_control;

pub mod binding {
    tonic::include_proto!("alia.binding");
}

pub mod body {
    tonic::include_proto!("alia.body.v1");
}

pub mod plugin {
    tonic::include_proto!("alia.plugin");
}

pub mod settings {
    tonic::include_proto!("alia.settings");
}

pub mod sensor {
    tonic::include_proto!("alia.sensor");
}
