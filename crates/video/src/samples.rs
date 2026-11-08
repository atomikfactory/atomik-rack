//! Realistic `ffprobe -print_format json -show_format -show_streams` outputs (trimmed to the
//! fields that matter), for the tests of this crate and of the app. They are what the programs
//! printed for small files; no file is shipped.

/// An H.264 video with one stereo AAC track in an MP4 file: 1920x1080 at 25 fps, 12 seconds.
pub const MP4_H264: &str = r#"{
    "streams": [
        {
            "index": 0,
            "codec_name": "h264",
            "codec_long_name": "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10",
            "profile": "High",
            "codec_type": "video",
            "width": 1920,
            "height": 1080,
            "pix_fmt": "yuv420p",
            "r_frame_rate": "25/1",
            "avg_frame_rate": "25/1",
            "time_base": "1/12800",
            "start_pts": 0,
            "start_time": "0.000000",
            "duration_ts": 153600,
            "duration": "12.000000",
            "nb_frames": "300",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "language": "und", "handler_name": "VideoHandler" }
        },
        {
            "index": 1,
            "codec_name": "aac",
            "codec_type": "audio",
            "sample_rate": "48000",
            "channels": 2,
            "channel_layout": "stereo",
            "start_time": "0.000000",
            "duration": "12.021333",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "language": "eng", "handler_name": "SoundHandler" }
        }
    ],
    "format": {
        "filename": "film.mp4",
        "nb_streams": 2,
        "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
        "start_time": "0.000000",
        "duration": "12.021333",
        "size": "2457600",
        "bit_rate": "1635301",
        "tags": { "major_brand": "isom", "encoder": "Lavf60.16.100" }
    }
}"#;

/// A ProRes video with 24-bit mono PCM and a timecode in a MOV file: 1920x1080 at 23.976 fps.
pub const MOV_PRORES: &str = r#"{
    "streams": [
        {
            "index": 0,
            "codec_name": "prores",
            "profile": "HQ",
            "codec_type": "video",
            "width": 1920,
            "height": 1080,
            "r_frame_rate": "24000/1001",
            "avg_frame_rate": "24000/1001",
            "start_time": "3600.000000",
            "duration": "10.427083",
            "nb_frames": "250",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "language": "eng", "timecode": "01:00:00:00" }
        },
        {
            "index": 1,
            "codec_name": "pcm_s24le",
            "codec_type": "audio",
            "sample_rate": "48000",
            "channels": 1,
            "start_time": "3600.000000",
            "duration": "10.427083",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "language": "eng" }
        },
        {
            "index": 2,
            "codec_name": "none",
            "codec_tag_string": "tmcd",
            "codec_type": "data",
            "r_frame_rate": "0/0",
            "avg_frame_rate": "0/0",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "timecode": "01:00:00:00" }
        }
    ],
    "format": {
        "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
        "start_time": "3600.000000",
        "duration": "10.427083",
        "size": "58122240",
        "tags": { "timecode": "01:00:00:00" }
    }
}"#;

/// A Matroska file: HEVC video whose duration is only in the container and in upper-case tags,
/// with an English and a French Opus track, 3840x2160 at 29.97 fps.
pub const MKV_HEVC: &str = r#"{
    "streams": [
        {
            "index": 0,
            "codec_name": "hevc",
            "codec_type": "video",
            "width": 3840,
            "height": 2160,
            "r_frame_rate": "30000/1001",
            "avg_frame_rate": "30000/1001",
            "start_time": "0.000000",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "DURATION": "00:01:30.063000000", "NUMBER_OF_FRAMES": "2700" }
        },
        {
            "index": 1,
            "codec_name": "opus",
            "codec_type": "audio",
            "sample_rate": "48000",
            "channels": 6,
            "channel_layout": "5.1",
            "start_time": "0.007000",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "language": "eng", "title": "Surround", "DURATION": "00:01:30.060000000" }
        },
        {
            "index": 2,
            "codec_name": "opus",
            "codec_type": "audio",
            "sample_rate": "48000",
            "channels": 2,
            "start_time": "0.007000",
            "disposition": { "default": 0, "attached_pic": 0 },
            "tags": { "language": "fre", "title": "Stereo (Francais)" }
        }
    ],
    "format": {
        "format_name": "matroska,webm",
        "start_time": "0.000000",
        "duration": "90.063000",
        "size": "104857600"
    }
}"#;

/// A silent film: one H.264 video stream and no audio, 1280x720 at 30 fps, 5 seconds.
pub const NO_AUDIO: &str = r#"{
    "streams": [
        {
            "index": 0,
            "codec_name": "h264",
            "codec_type": "video",
            "width": 1280,
            "height": 720,
            "r_frame_rate": "30/1",
            "avg_frame_rate": "30/1",
            "start_time": "0.000000",
            "duration": "5.000000",
            "nb_frames": "150",
            "disposition": { "default": 1, "attached_pic": 0 }
        }
    ],
    "format": {
        "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
        "start_time": "0.000000",
        "duration": "5.000000"
    }
}"#;

/// A phone film: 1920x1080 stored sideways with a display matrix that turns it upright (so it
/// shows as 1080x1920), variable frame rate (average 29.83, container rate 600 fps) and two
/// audio tracks.
pub const PHONE_ROTATED: &str = r#"{
    "streams": [
        {
            "index": 0,
            "codec_name": "h264",
            "codec_type": "video",
            "width": 1920,
            "height": 1080,
            "r_frame_rate": "600/1",
            "avg_frame_rate": "2983/100",
            "start_time": "0.000000",
            "duration": "20.000000",
            "nb_frames": "597",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "rotate": "90" },
            "side_data_list": [
                { "side_data_type": "Display Matrix", "displaymatrix": "\n00000000:            0       65536           0\n", "rotation": -90 }
            ]
        },
        {
            "index": 1,
            "codec_name": "aac",
            "codec_type": "audio",
            "sample_rate": "44100",
            "channels": 2,
            "start_time": "0.000000",
            "disposition": { "default": 1, "attached_pic": 0 },
            "tags": { "language": "eng" }
        },
        {
            "index": 2,
            "codec_name": "aac",
            "codec_type": "audio",
            "sample_rate": "44100",
            "channels": 1,
            "start_time": "0.000000",
            "disposition": { "default": 0, "attached_pic": 0 },
            "tags": { "language": "spa", "title": "Comentario" }
        }
    ],
    "format": {
        "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
        "start_time": "0.000000",
        "duration": "20.020000"
    }
}"#;

/// An audio file with cover art: the only video stream is a still picture, so there is no film.
pub const COVER_ART_ONLY: &str = r#"{
    "streams": [
        {
            "index": 0,
            "codec_name": "mp3",
            "codec_type": "audio",
            "sample_rate": "44100",
            "channels": 2,
            "start_time": "0.025057",
            "duration": "200.000000",
            "disposition": { "default": 0, "attached_pic": 0 }
        },
        {
            "index": 1,
            "codec_name": "mjpeg",
            "codec_type": "video",
            "width": 600,
            "height": 600,
            "r_frame_rate": "90000/1",
            "avg_frame_rate": "0/0",
            "disposition": { "default": 0, "attached_pic": 1 }
        }
    ],
    "format": {
        "format_name": "mp3",
        "start_time": "0.025057",
        "duration": "200.000000"
    }
}"#;
