//! Tests for the audio inspection RPC surface: `wav`, `audio_state`,
//! `audio_events`, `audio_stats` and `spectrogram`, plus the audio-log
//! replay guarantee for `load_state`.

use std::collections::HashSet;
use std::fs;

use console_agent::rpc::handle;
use console_agent::session::Session;
use console_core::SAMPLES_PER_FRAME;
use serde_json::json;

fn demo_cart_text() -> String {
    let path = format!("{}/../../carts/demo.cart", env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "console-agent-audio-test-{}-{name}",
        std::process::id()
    ))
}

fn load(session: &mut Session, seed: u64) {
    let resp = handle(
        session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": demo_cart_text(), "seed": seed}}),
    );
    assert!(resp.get("error").is_none(), "load_cart failed: {resp}");
}

fn step(session: &mut Session, frames: u64, input: &str) -> serde_json::Value {
    handle(
        session,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": frames, "input": input}}),
    )
}

/// Decode a hand-rolled WAV file back into i16 samples, checking the header
/// along the way (RIFF/WAVE/fmt /data chunk ids and declared sizes).
fn decode_wav(bytes: &[u8]) -> Vec<i16> {
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(&bytes[12..16], b"fmt ");
    assert_eq!(&bytes[36..40], b"data");

    let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
    assert_eq!(riff_size, 36 + data_size, "RIFF chunk size mismatch");
    assert_eq!(bytes.len(), 44 + data_size as usize, "file size mismatch");

    let channels = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
    assert_eq!(channels, 1, "expected mono");
    let sample_rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    assert_eq!(sample_rate, console_core::SAMPLE_RATE);
    let bits_per_sample = u16::from_le_bytes(bytes[34..36].try_into().unwrap());
    assert_eq!(bits_per_sample, 16);

    bytes[44..]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[test]
fn wav_has_valid_header_and_nonzero_rms() {
    let mut session = Session::new();
    load(&mut session, 1);
    let resp = step(&mut session, 120, "");
    assert_eq!(resp["result"]["frame_count"], 120);

    let path = temp_path("out.wav");
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "wav", "params": {"path": path.to_str().unwrap()}}),
    );
    assert!(resp.get("error").is_none(), "wav failed: {resp}");
    assert_eq!(resp["result"]["frames"], 120);
    assert_eq!(resp["result"]["samples"], 120 * SAMPLES_PER_FRAME as u64);
    let expected_duration = 120.0 * SAMPLES_PER_FRAME as f64 / f64::from(console_core::SAMPLE_RATE);
    let got_duration = resp["result"]["duration_seconds"].as_f64().unwrap();
    assert!((got_duration - expected_duration).abs() < 1e-9);

    let bytes = fs::read(&path).expect("wav file was written");
    let samples = decode_wav(&bytes);
    assert_eq!(samples.len(), 120 * SAMPLES_PER_FRAME);

    // Music starts in `_init`, so a 120-frame run should not be silent.
    let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    assert!(
        rms > 0.0,
        "expected nonzero RMS for demo cart music, got {rms}"
    );

    let _ = fs::remove_file(&path);
}

#[test]
fn events_include_music_note_on_at_frame_one_and_button_blip() {
    let mut session = Session::new();
    load(&mut session, 1);

    // Frame 1: music() was started in `_init`, before any step, so the
    // very first frame's diff (against the idle baseline) should fire
    // note_on for both music channels.
    step(&mut session, 1, "");
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "audio_events", "params": {}}),
    );
    let events = resp["result"].as_array().expect("events array");
    let music_note_ons: Vec<_> = events
        .iter()
        .filter(|e| e["frame"] == 1 && e["kind"] == "note_on" && e["from_music"] == true)
        .collect();
    assert!(
        !music_note_ons.is_empty(),
        "expected music note_on events at frame 1, got {events:#?}"
    );

    // A few more silent frames, then press A once: `_update` calls
    // sfx(4) (the "bright blip") on btnp(4), auto-picking a non-music
    // channel.
    step(&mut session, 5, "");
    let resp = step(&mut session, 1, "A");
    let press_frame = resp["result"]["frame_count"].as_u64().unwrap();
    assert_eq!(press_frame, 7);

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 4, "method": "audio_events", "params": {"from_frame": press_frame.saturating_sub(1)}}),
    );
    let events = resp["result"].as_array().expect("events array");
    let blip = events
        .iter()
        .find(|e| e["kind"] == "note_on" && e["sfx"] == 4 && e["from_music"] == false);
    assert!(
        blip.is_some(),
        "expected a note_on for sfx 4 (blip) with from_music=false near frame {press_frame}, got {events:#?}"
    );
    let blip_frame = blip.unwrap()["frame"].as_u64().unwrap();
    assert!(
        blip_frame.abs_diff(press_frame) <= 2,
        "blip note_on at frame {blip_frame} should be within a frame or two of the press at {press_frame}"
    );
}

#[test]
fn audio_state_reports_busy_music_channels() {
    let mut session = Session::new();
    load(&mut session, 1);
    step(&mut session, 30, "");

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "audio_state", "params": {}}),
    );
    assert!(resp.get("error").is_none(), "audio_state failed: {resp}");
    let channels = resp["result"]["channels"]
        .as_array()
        .expect("channels array");
    assert_eq!(channels.len(), 4);
    assert_eq!(channels[0]["busy"], true);
    assert_eq!(channels[0]["from_music"], true);
    assert_eq!(channels[1]["busy"], true);
    assert_eq!(channels[1]["from_music"], true);
    assert_eq!(resp["result"]["frame_count"], 30);
    assert!(resp["result"]["music_pattern"].is_number());
}

#[test]
fn stats_window_count_and_no_clipping() {
    let mut session = Session::new();
    load(&mut session, 1);
    step(&mut session, 125, ""); // not an exact multiple of the window size

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "audio_stats", "params": {}}),
    );
    assert!(resp.get("error").is_none(), "audio_stats failed: {resp}");
    let windows = resp["result"].as_array().expect("windows array");
    assert_eq!(windows.len(), (125f64 / 6.0).ceil() as usize);

    let mut any_rms_positive = false;
    for w in windows {
        assert_eq!(w["clipped"], 0, "demo cart audio should never clip: {w}");
        if w["rms"].as_f64().unwrap() > 0.0 {
            any_rms_positive = true;
        }
    }
    assert!(any_rms_positive, "expected at least one window with rms>0");
}

#[test]
fn spectrogram_dimensions_and_bass_orientation() {
    let mut session = Session::new();
    load(&mut session, 1);
    step(&mut session, 180, "");

    let path = temp_path("spectrogram.png");
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "spectrogram", "params": {"path": path.to_str().unwrap()}}),
    );
    assert!(resp.get("error").is_none(), "spectrogram failed: {resp}");

    let windows = resp["result"]["windows"].as_u64().unwrap() as usize;
    let width = resp["result"]["width"].as_u64().unwrap() as u32;
    let height = resp["result"]["height"].as_u64().unwrap() as u32;
    // 3-frame windows, default cell=4, 96 semitone rows.
    let expected_windows = (180 * SAMPLES_PER_FRAME).div_ceil(3 * SAMPLES_PER_FRAME);
    assert_eq!(windows, expected_windows);
    assert_eq!(width, windows as u32 * 4);
    assert_eq!(height, 96 * 4);

    let decoder = png::Decoder::new(std::io::BufReader::new(
        fs::File::open(&path).expect("spectrogram file exists"),
    ));
    let mut reader = decoder.read_info().expect("valid png header");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("known buffer size")];
    let info = reader.next_frame(&mut buf).expect("decode png frame");
    assert_eq!(info.width, width);
    assert_eq!(info.height, height);
    let pixels = &buf[..info.buffer_size()];
    let channels = info.color_type.samples();

    let mut colors: HashSet<&[u8]> = HashSet::new();
    for px in pixels.chunks_exact(channels) {
        colors.insert(px);
    }
    assert!(
        colors.len() >= 3,
        "expected nonzero pixel variance in the spectrogram, got {} distinct colors",
        colors.len()
    );

    // Sanity check the orientation isn't flipped: B7 at the top, C0 at the
    // bottom. The demo tune's bass channel plays octaves 2-3 and its melody
    // plays octaves 4-5; nothing plays in octave 7 (the very top rows), so
    // the bass region should read brighter (higher average byte value) than
    // the top-octave region.
    let cell = 4u32;
    let row_brightness = |note_row: u32| -> f64 {
        let y0 = note_row * cell;
        let mut sum = 0u64;
        let mut count = 0u64;
        for y in y0..y0 + cell {
            for x in 0..width {
                let idx = ((y * width + x) * channels as u32) as usize;
                for c in 0..3 {
                    sum += u64::from(pixels[idx + c]);
                    count += 1;
                }
            }
        }
        sum as f64 / count as f64
    };

    // B7 at row 0 (top) .. C0 at row 95 (bottom). Octave 1-2 (notes 12..36)
    // occupy rows (95-35)..=(95-12) = 60..=83; octave 7 (notes 84..96)
    // occupies rows 0..=11.
    let bass_rows = 60..84;
    let top_octave_rows = 0..12;
    let bass_brightness: f64 =
        bass_rows.clone().map(row_brightness).sum::<f64>() / bass_rows.len() as f64;
    let top_brightness: f64 =
        top_octave_rows.clone().map(row_brightness).sum::<f64>() / top_octave_rows.len() as f64;
    assert!(
        bass_brightness > top_brightness,
        "expected bass region ({bass_brightness}) brighter than top octave ({top_brightness}) \
         for the demo tune -- spectrogram orientation may be flipped"
    );

    let _ = fs::remove_file(&path);
}

#[test]
fn load_state_reproduces_continuous_audio_log() {
    // Continuous 90-frame run.
    let mut continuous = Session::new();
    load(&mut continuous, 7);
    step(&mut continuous, 90, "R");
    let continuous_log = continuous.audio_log().to_vec();
    assert_eq!(continuous_log.len(), 90 * SAMPLES_PER_FRAME);

    // Step 60, save, step 30 more (to be undone), load, replay identically.
    let mut split = Session::new();
    load(&mut split, 7);
    step(&mut split, 60, "R");
    let resp = handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 3, "method": "save_state", "params": {"name": "mid"}}),
    );
    assert_eq!(resp["result"]["ok"], true);

    step(&mut split, 30, "R");

    let resp = handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 5, "method": "load_state", "params": {"name": "mid"}}),
    );
    assert_eq!(resp["result"]["replayed_frames"], 60);

    step(&mut split, 30, "R");

    let split_log = split.audio_log().to_vec();
    assert_eq!(
        continuous_log, split_log,
        "replayed audio log must be byte-identical to the continuous run"
    );
}
