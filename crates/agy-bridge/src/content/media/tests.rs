use super::*;

#[test]
fn image_struct_serde_roundtrip() {
    let img = Image {
        data: vec![10, 20, 30],
        mime_type: "image/bmp".to_string(),
        description: Some("bitmap".to_string()),
    };
    let json = serde_json::to_string(&img).unwrap();
    let parsed: Image = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, img);
}

#[test]
fn document_struct_serde_roundtrip() {
    let doc = Document {
        data: b"{}".to_vec(),
        mime_type: "application/json".to_string(),
        description: None,
    };
    let json = serde_json::to_string(&doc).unwrap();
    let parsed: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, doc);
}

#[test]
fn audio_struct_serde_roundtrip() {
    let audio = Audio {
        data: vec![0xAA, 0xBB],
        mime_type: "audio/wav".to_string(),
        description: Some("beep".to_string()),
    };
    let json = serde_json::to_string(&audio).unwrap();
    let parsed: Audio = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, audio);
}

#[test]
fn video_struct_serde_roundtrip() {
    let video = Video {
        data: vec![0xCC, 0xDD, 0xEE],
        mime_type: "video/webm".to_string(),
        description: None,
    };
    let json = serde_json::to_string(&video).unwrap();
    let parsed: Video = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, video);
}

#[test]
fn image_description_defaults_to_none() {
    let json = r#"{"data":[1,2,3],"mime_type":"image/png"}"#;
    let img: Image = serde_json::from_str(json).unwrap();
    assert!(img.description.is_none());
}

#[test]
fn image_new_creates_correct_image() {
    let img = Image::new(vec![10, 20], "image/webp");
    assert_eq!(img.data, vec![10, 20]);
    assert_eq!(img.mime_type, "image/webp");
    assert!(img.description.is_none());
}

#[test]
fn image_png_creates_correct_image() {
    let img = Image::png(vec![1, 2, 3]);
    assert_eq!(img.data, vec![1, 2, 3]);
    assert_eq!(img.mime_type, "image/png");
    assert!(img.description.is_none());
}

#[test]
fn image_jpeg_creates_correct_image() {
    let img = Image::jpeg(vec![0xFF, 0xD8]);
    assert_eq!(img.data, vec![0xFF, 0xD8]);
    assert_eq!(img.mime_type, "image/jpeg");
    assert!(img.description.is_none());
}

#[test]
fn document_new_creates_correct_document() {
    let doc = Document::new(b"data".to_vec(), "text/plain");
    assert_eq!(doc.data, b"data".to_vec());
    assert_eq!(doc.mime_type, "text/plain");
    assert!(doc.description.is_none());
}

#[test]
fn document_pdf_creates_correct_document() {
    let doc = Document::pdf(b"%PDF-1.4".to_vec());
    assert_eq!(doc.data, b"%PDF-1.4".to_vec());
    assert_eq!(doc.mime_type, "application/pdf");
    assert!(doc.description.is_none());
}

#[test]
fn audio_new_creates_correct_audio() {
    let audio = Audio::new(vec![0xAA], "audio/ogg");
    assert_eq!(audio.data, vec![0xAA]);
    assert_eq!(audio.mime_type, "audio/ogg");
    assert!(audio.description.is_none());
}

#[test]
fn audio_mp3_creates_correct_audio() {
    let audio = Audio::mp3(vec![0xFF, 0xFB]);
    assert_eq!(audio.data, vec![0xFF, 0xFB]);
    assert_eq!(audio.mime_type, "audio/mpeg");
    assert!(audio.description.is_none());
}

#[test]
fn audio_wav_creates_correct_audio() {
    let audio = Audio::wav(vec![0x52, 0x49, 0x46, 0x46]);
    assert_eq!(audio.data, vec![0x52, 0x49, 0x46, 0x46]);
    assert_eq!(audio.mime_type, "audio/wav");
    assert!(audio.description.is_none());
}

#[test]
fn video_new_creates_correct_video() {
    let video = Video::new(vec![0x00], "video/webm");
    assert_eq!(video.data, vec![0x00]);
    assert_eq!(video.mime_type, "video/webm");
    assert!(video.description.is_none());
}

#[test]
fn video_mp4_creates_correct_video() {
    let video = Video::mp4(vec![0x00, 0x00, 0x00, 0x1C]);
    assert_eq!(video.data, vec![0x00, 0x00, 0x00, 0x1C]);
    assert_eq!(video.mime_type, "video/mp4");
    assert!(video.description.is_none());
}

#[test]
fn image_new_accepts_string_type() {
    let img = Image::new(vec![1], String::from("image/bmp"));
    assert_eq!(img.mime_type, "image/bmp");
}

// ── with_description() builder ──────────────────────────────────

#[test]
fn image_with_description_sets_description() {
    let img = Image::png(vec![1]).with_description("a logo");
    assert_eq!(img.description.as_deref(), Some("a logo"));
    assert_eq!(img.mime_type, "image/png");
}

#[test]
fn document_with_description_sets_description() {
    let doc = Document::pdf(vec![1]).with_description("invoice");
    assert_eq!(doc.description.as_deref(), Some("invoice"));
    assert_eq!(doc.mime_type, "application/pdf");
}

#[test]
fn audio_with_description_sets_description() {
    let audio = Audio::mp3(vec![1]).with_description("intro jingle");
    assert_eq!(audio.description.as_deref(), Some("intro jingle"));
    assert_eq!(audio.mime_type, "audio/mpeg");
}

#[test]
fn video_with_description_sets_description() {
    let video = Video::mp4(vec![1]).with_description("demo clip");
    assert_eq!(video.description.as_deref(), Some("demo clip"));
    assert_eq!(video.mime_type, "video/mp4");
}

// ── Convenience constructors ────────────────────────────────────

#[test]
fn image_webp_creates_correct_image() {
    let img = Image::webp(vec![1, 2]);
    assert_eq!(img.mime_type, "image/webp");
    assert_eq!(img.data, vec![1, 2]);
    assert!(img.description.is_none());
}

#[test]
fn image_bmp_creates_correct_image() {
    let img = Image::bmp(vec![0x42, 0x4D]);
    assert_eq!(img.mime_type, "image/bmp");
    assert_eq!(img.data, vec![0x42, 0x4D]);
    assert!(img.description.is_none());
}

#[test]
fn convenience_constructors_use_sdk_allowed_mimes() {
    // Mirrors google.antigravity.types SUPPORTED_*_MIMES (the source of
    // truth). If the SDK allowlist changes, update these arrays and the
    // constructors together.
    const SDK_IMAGE: &[&str] = &["image/bmp", "image/jpeg", "image/png", "image/webp"];
    const SDK_DOCUMENT: &[&str] = &[
        "application/pdf",
        "application/json",
        "text/css",
        "text/csv",
        "text/html",
        "text/javascript",
        "text/plain",
        "text/rtf",
        "text/xml",
    ];
    const SDK_AUDIO: &[&str] = &[
        "audio/wav",
        "audio/mp3",
        "audio/aac",
        "audio/ogg",
        "audio/flac",
        "audio/opus",
        "audio/mpeg",
        "audio/m4a",
        "audio/l16",
    ];
    const SDK_VIDEO: &[&str] = &[
        "video/3gpp",
        "video/avi",
        "video/mp4",
        "video/mpeg",
        "video/mpg",
        "video/quicktime",
        "video/webm",
        "video/wmv",
        "video/x-flv",
    ];

    let d = vec![0u8];
    for m in [
        Image::png(d.clone()).mime_type,
        Image::jpeg(d.clone()).mime_type,
        Image::webp(d.clone()).mime_type,
        Image::bmp(d.clone()).mime_type,
    ] {
        assert!(
            SDK_IMAGE.contains(&m.as_str()),
            "image constructor mime {m} not in SDK allowlist"
        );
    }
    for m in [
        Document::pdf(d.clone()).mime_type,
        Document::json(d.clone()).mime_type,
        Document::plain_text(d.clone()).mime_type,
    ] {
        assert!(
            SDK_DOCUMENT.contains(&m.as_str()),
            "document constructor mime {m} not in SDK allowlist"
        );
    }
    for m in [
        Audio::mp3(d.clone()).mime_type,
        Audio::wav(d.clone()).mime_type,
        Audio::ogg(d.clone()).mime_type,
        Audio::flac(d.clone()).mime_type,
    ] {
        assert!(
            SDK_AUDIO.contains(&m.as_str()),
            "audio constructor mime {m} not in SDK allowlist"
        );
    }
    for m in [
        Video::mp4(d.clone()).mime_type,
        Video::webm(d.clone()).mime_type,
    ] {
        assert!(
            SDK_VIDEO.contains(&m.as_str()),
            "video constructor mime {m} not in SDK allowlist"
        );
    }

    // Every extension the bridge infers must map to an SDK-allowed MIME.
    for ext in [
        "png", "jpg", "jpeg", "bmp", "webp", "pdf", "txt", "json", "css", "csv", "html", "htm",
        "js", "mjs", "rtf", "xml", "mp3", "wav", "ogg", "oga", "flac", "aac", "opus", "m4a", "mp4",
        "m4v", "webm", "3gp", "3gpp", "avi", "mpeg", "mpg", "mov", "wmv", "flv",
    ] {
        let mime = mime::from_extension(ext).expect("known extension");
        let allowed = SDK_IMAGE.contains(&mime)
            || SDK_DOCUMENT.contains(&mime)
            || SDK_AUDIO.contains(&mime)
            || SDK_VIDEO.contains(&mime);
        assert!(allowed, "extension .{ext} maps to non-SDK mime {mime}");
    }
}

#[test]
fn audio_ogg_creates_correct_audio() {
    let audio = Audio::ogg(vec![0x4F, 0x67]);
    assert_eq!(audio.mime_type, "audio/ogg");
    assert_eq!(audio.data, vec![0x4F, 0x67]);
    assert!(audio.description.is_none());
}

#[test]
fn audio_flac_creates_correct_audio() {
    let audio = Audio::flac(vec![0x66, 0x4C]);
    assert_eq!(audio.mime_type, "audio/flac");
    assert_eq!(audio.data, vec![0x66, 0x4C]);
    assert!(audio.description.is_none());
}

#[test]
fn document_plain_text_creates_correct_document() {
    let doc = Document::plain_text(b"hello".to_vec());
    assert_eq!(doc.mime_type, "text/plain");
    assert_eq!(doc.data, b"hello");
    assert!(doc.description.is_none());
}

#[test]
fn document_json_creates_correct_document() {
    let doc = Document::json(b"{}".to_vec());
    assert_eq!(doc.mime_type, "application/json");
    assert_eq!(doc.data, b"{}");
    assert!(doc.description.is_none());
}

#[test]
fn video_webm_creates_correct_video() {
    let video = Video::webm(vec![0x1A, 0x45]);
    assert_eq!(video.mime_type, "video/webm");
    assert_eq!(video.data, vec![0x1A, 0x45]);
    assert!(video.description.is_none());
}

// ── from_file() — Image ────────────────────────────────────────

#[test]
fn image_from_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.png");
    std::fs::write(&path, b"\x89PNG").unwrap();
    let img = Image::from_file(&path).unwrap();
    assert_eq!(img.data, b"\x89PNG");
    assert_eq!(img.mime_type, "image/png");
    assert!(img.description.is_none());
}

#[test]
fn image_from_file_unknown_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.tiff");
    std::fs::write(&path, b"II").unwrap();
    let err = Image::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("unrecognized"),
        "expected 'unrecognized' in: {err}"
    );
}

#[test]
fn image_from_file_wrong_mime_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not_image.mp3");
    std::fs::write(&path, b"\xFF\xFB").unwrap();
    let err = Image::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("not an image type"),
        "expected MIME prefix error in: {err}"
    );
}

#[test]
fn image_from_file_missing_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noext");
    std::fs::write(&path, b"data").unwrap();
    let err = Image::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("missing file extension"),
        "expected 'missing file extension' in: {err}"
    );
}

// ── from_file() — Document ─────────────────────────────────────

#[test]
fn document_from_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.pdf");
    std::fs::write(&path, b"%PDF-1.4").unwrap();
    let doc = Document::from_file(&path).unwrap();
    assert_eq!(doc.data, b"%PDF-1.4");
    assert_eq!(doc.mime_type, "application/pdf");
    assert!(doc.description.is_none());
}

#[test]
fn document_from_file_text_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"hello world").unwrap();
    let doc = Document::from_file(&path).unwrap();
    assert_eq!(doc.data, b"hello world");
    assert_eq!(doc.mime_type, "text/plain");
}

#[test]
fn document_from_file_json_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, b"{}").unwrap();
    let doc = Document::from_file(&path).unwrap();
    assert_eq!(doc.data, b"{}");
    assert_eq!(doc.mime_type, "application/json");
}

#[test]
fn document_from_file_unknown_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.xyz");
    std::fs::write(&path, b"stuff").unwrap();
    let err = Document::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("unrecognized"),
        "expected 'unrecognized' in: {err}"
    );
}

#[test]
fn document_from_file_wrong_mime_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not_doc.png");
    std::fs::write(&path, b"\x89PNG").unwrap();
    let err = Document::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("not a document type"),
        "expected MIME prefix error in: {err}"
    );
}

#[test]
fn document_from_file_missing_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noext");
    std::fs::write(&path, b"data").unwrap();
    let err = Document::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("missing file extension"),
        "expected 'missing file extension' in: {err}"
    );
}

// ── from_file() — Audio ────────────────────────────────────────

#[test]
fn audio_from_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp3");
    std::fs::write(&path, b"\xFF\xFB\x90").unwrap();
    let audio = Audio::from_file(&path).unwrap();
    assert_eq!(audio.data, b"\xFF\xFB\x90");
    assert_eq!(audio.mime_type, "audio/mpeg");
    assert!(audio.description.is_none());
}

#[test]
fn audio_from_file_wav_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.wav");
    std::fs::write(&path, b"RIFF").unwrap();
    let audio = Audio::from_file(&path).unwrap();
    assert_eq!(audio.mime_type, "audio/wav");
}

#[test]
fn audio_from_file_unknown_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sound.mid");
    std::fs::write(&path, b"data").unwrap();
    let err = Audio::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("unrecognized"),
        "expected 'unrecognized' in: {err}"
    );
}

#[test]
fn audio_from_file_wrong_mime_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not_audio.png");
    std::fs::write(&path, b"\x89PNG").unwrap();
    let err = Audio::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("not an audio type"),
        "expected MIME prefix error in: {err}"
    );
}

#[test]
fn audio_from_file_missing_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noext");
    std::fs::write(&path, b"data").unwrap();
    let err = Audio::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("missing file extension"),
        "expected 'missing file extension' in: {err}"
    );
}

// ── from_file() — Video ────────────────────────────────────────

#[test]
fn video_from_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    std::fs::write(&path, b"\x00\x00\x00\x1Cftyp").unwrap();
    let video = Video::from_file(&path).unwrap();
    assert_eq!(video.data, b"\x00\x00\x00\x1Cftyp");
    assert_eq!(video.mime_type, "video/mp4");
    assert!(video.description.is_none());
}

#[test]
fn video_from_file_webm_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.webm");
    std::fs::write(&path, b"\x1A\x45\xDF\xA3").unwrap();
    let video = Video::from_file(&path).unwrap();
    assert_eq!(video.mime_type, "video/webm");
}

#[test]
fn video_from_file_unknown_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("movie.mkv");
    std::fs::write(&path, b"RIFF").unwrap();
    let err = Video::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("unrecognized"),
        "expected 'unrecognized' in: {err}"
    );
}

#[test]
fn video_from_file_wrong_mime_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not_video.png");
    std::fs::write(&path, b"\x89PNG").unwrap();
    let err = Video::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("not a video type"),
        "expected MIME prefix error in: {err}"
    );
}

#[test]
fn video_from_file_missing_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noext");
    std::fs::write(&path, b"data").unwrap();
    let err = Video::from_file(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("missing file extension"),
        "expected 'missing file extension' in: {err}"
    );
}

// ── from_file() — file does not exist ──────────────────────────

#[test]
fn image_from_file_nonexistent_file() {
    let err = Image::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.png").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn document_from_file_nonexistent_file() {
    let err = Document::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.pdf").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn audio_from_file_nonexistent_file() {
    let err = Audio::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.mp3").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn video_from_file_nonexistent_file() {
    let err = Video::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.mp4").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

// ── mime::from_extension ────────────────────────────────────────

#[test]
fn mime_from_extension_case_insensitive() {
    assert_eq!(mime::from_extension("PNG"), Some("image/png"));
    assert_eq!(mime::from_extension("Jpeg"), Some("image/jpeg"));
    assert_eq!(mime::from_extension("MP4"), Some("video/mp4"));
}

#[test]
fn mime_from_extension_unknown_returns_none() {
    assert_eq!(mime::from_extension("tiff"), None);
    assert_eq!(mime::from_extension("tga"), None);
    assert_eq!(mime::from_extension(""), None);
}
