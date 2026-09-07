pub mod audio {
    tonic::include_proto!("alia.audio");
}

pub mod voice {
    tonic::include_proto!("alia.voice.v1");
}

pub mod playback {
    tonic::include_proto!("alia.playback.v1");
}

pub mod binding {
    tonic::include_proto!("alia.binding");
}

pub mod expression {
    tonic::include_proto!("alia.expression");
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
