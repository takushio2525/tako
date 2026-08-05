//! テスト用の mp4（H.264 + AAC）を **OS の Media Foundation エンコーダだけ**で作る。
//!
//! ffmpeg も素材ファイルもリポジトリに要らないので、開発機でも CI の Windows ランナーでも
//! 同じ検証が回る。ついでに「この環境に H.264 / AAC のコーデックがある」ことの確認にもなる
//! （無ければ生成に失敗し、呼び出し側のテストはスキップする）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFSample, IMFSinkWriter, MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio,
    MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE,
};

const W: u32 = 320;
const H: u32 = 240;
const FPS: u32 = 10;
const SECONDS: u32 = 6;
const SAMPLE_RATE: u32 = 44100;
const CHANNELS: u32 = 2;

/// 検証用 mp4（映像 + 音声）のパス。生成できない環境では `None`（呼び出し側はスキップする）。
///
/// プロセスで 1 回だけ作って使い回す（エンコードに 1 秒前後かかるため）。
pub(super) fn sample_mp4() -> Option<PathBuf> {
    static FIXTURE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FIXTURE.get_or_init(|| build("sample.mp4", true)).clone()
}

/// 音声トラックだけの mp4。フレームが永久に来ない経路（`has_video = false`）を突く
pub(super) fn audio_only_mp4() -> Option<PathBuf> {
    static FIXTURE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FIXTURE
        .get_or_init(|| build("audio-only.mp4", false))
        .clone()
}

fn build(name: &str, with_video: bool) -> Option<PathBuf> {
    super::windows::ensure_media_foundation().ok()?;
    let dir = std::env::temp_dir().join("tako-video-fixture");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    match unsafe { write_mp4(&path, with_video) } {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!("検証用 mp4 を生成できない: {e}");
            None
        }
    }
}

/// 1 秒ごとに色が変わる映像 + 440Hz のサイン波を H.264 / AAC で書き出す。
/// `with_video = false` なら映像トラックを丸ごと省いて音声だけの mp4 にする
unsafe fn write_mp4(path: &Path, with_video: bool) -> Result<(), String> {
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let writer: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, None)
        .map_err(|e| format!("mp4 を作成できない: {e}"))?;

    let vstream = if with_video {
        Some(add_video_stream(&writer)?)
    } else {
        None
    };
    let astream = add_audio_stream(&writer)?;

    writer.BeginWriting().map_err(hr)?;
    if let Some(vstream) = vstream {
        write_video(&writer, vstream)?;
    }
    write_audio(&writer, astream)?;
    writer.Finalize().map_err(hr)?;
    Ok(())
}

/// 映像ストリーム（出力 H.264 / 入力 RGB32）を足してストリーム番号を返す
unsafe fn add_video_stream(writer: &IMFSinkWriter) -> Result<u32, String> {
    let out = MFCreateMediaType().map_err(hr)?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(hr)?;
    out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
        .map_err(hr)?;
    out.SetUINT32(&MF_MT_AVG_BITRATE, 800_000).map_err(hr)?;
    out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
        .map_err(hr)?;
    out.SetUINT64(&MF_MT_FRAME_SIZE, pack(W, H)).map_err(hr)?;
    out.SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1)).map_err(hr)?;
    out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
        .map_err(hr)?;
    let stream = writer.AddStream(&out).map_err(hr)?;

    let input = MFCreateMediaType().map_err(hr)?;
    input
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(hr)?;
    input
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
        .map_err(hr)?;
    input
        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
        .map_err(hr)?;
    input.SetUINT64(&MF_MT_FRAME_SIZE, pack(W, H)).map_err(hr)?;
    input
        .SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1))
        .map_err(hr)?;
    input
        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
        .map_err(hr)?;
    writer.SetInputMediaType(stream, &input, None).map_err(hr)?;
    Ok(stream)
}

/// 音声ストリーム（出力 AAC / 入力 PCM）を足してストリーム番号を返す
unsafe fn add_audio_stream(writer: &IMFSinkWriter) -> Result<u32, String> {
    let out = MFCreateMediaType().map_err(hr)?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
        .map_err(hr)?;
    out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
        .map_err(hr)?;
    out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
        .map_err(hr)?;
    out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)
        .map_err(hr)?;
    out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)
        .map_err(hr)?;
    out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 16_000)
        .map_err(hr)?;
    let stream = writer.AddStream(&out).map_err(hr)?;

    let input = MFCreateMediaType().map_err(hr)?;
    input
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
        .map_err(hr)?;
    input
        .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
        .map_err(hr)?;
    input
        .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
        .map_err(hr)?;
    input
        .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)
        .map_err(hr)?;
    input
        .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)
        .map_err(hr)?;
    input
        .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2)
        .map_err(hr)?;
    input
        .SetUINT32(
            &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            SAMPLE_RATE * CHANNELS * 2,
        )
        .map_err(hr)?;
    writer.SetInputMediaType(stream, &input, None).map_err(hr)?;
    Ok(stream)
}

/// 1 秒ごとに色が変わり、下端の進行バーが伸びる映像を書く
/// （シークで「絵が変わった」ことを検出できるようにするため）
unsafe fn write_video(writer: &IMFSinkWriter, stream: u32) -> Result<(), String> {
    let total_frames = FPS * SECONDS;
    let frame_dur = 10_000_000i64 / FPS as i64;
    for i in 0..total_frames {
        let mut pixels = vec![0u8; (W * H * 4) as usize];
        let (r, g, b) = match (i / FPS) % 6 {
            0 => (220u8, 40u8, 40u8),
            1 => (220, 160, 40),
            2 => (60, 200, 60),
            3 => (40, 160, 220),
            4 => (140, 60, 220),
            _ => (230, 230, 230),
        };
        let bar = ((i as f32 / total_frames as f32) * W as f32) as u32;
        for y in 0..H {
            for x in 0..W {
                let o = ((y * W + x) * 4) as usize;
                let lit = y > H - 20 && x < bar;
                // RGB32 は BGRX のバイト順
                pixels[o] = if lit { 255 } else { b };
                pixels[o + 1] = if lit { 255 } else { g };
                pixels[o + 2] = if lit { 255 } else { r };
                pixels[o + 3] = 255;
            }
        }
        let sample = make_sample(&pixels, i as i64 * frame_dur, frame_dur)?;
        writer.WriteSample(stream, &sample).map_err(hr)?;
    }
    Ok(())
}

/// 440Hz のサイン波を 100ms ずつ書く
unsafe fn write_audio(writer: &IMFSinkWriter, stream: u32) -> Result<(), String> {
    let total_samples = SAMPLE_RATE * SECONDS;
    let chunk = SAMPLE_RATE / 10;
    let mut written = 0u32;
    while written < total_samples {
        let n = chunk.min(total_samples - written);
        let mut pcm = vec![0u8; (n * CHANNELS * 2) as usize];
        for k in 0..n {
            let t = (written + k) as f64 / SAMPLE_RATE as f64;
            let v = ((t * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16;
            for c in 0..CHANNELS {
                let o = ((k * CHANNELS + c) * 2) as usize;
                pcm[o..o + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        let t_hns = written as i64 * 10_000_000 / SAMPLE_RATE as i64;
        let d_hns = n as i64 * 10_000_000 / SAMPLE_RATE as i64;
        let sample = make_sample(&pcm, t_hns, d_hns)?;
        writer.WriteSample(stream, &sample).map_err(hr)?;
        written += n;
    }
    Ok(())
}

unsafe fn make_sample(data: &[u8], time_hns: i64, dur_hns: i64) -> Result<IMFSample, String> {
    let sample = MFCreateSample().map_err(hr)?;
    let buffer = MFCreateMemoryBuffer(data.len() as u32).map_err(hr)?;
    let mut ptr = std::ptr::null_mut();
    buffer.Lock(&mut ptr, None, None).map_err(hr)?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
    buffer.Unlock().map_err(hr)?;
    buffer.SetCurrentLength(data.len() as u32).map_err(hr)?;
    sample.AddBuffer(&buffer).map_err(hr)?;
    sample.SetSampleTime(time_hns).map_err(hr)?;
    sample.SetSampleDuration(dur_hns).map_err(hr)?;
    Ok(sample)
}

/// メディア型の「サイズ」「フレームレート」は 64bit に 2 つの 32bit を詰めて表す
fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn hr(e: windows::core::Error) -> String {
    format!("Media Foundation エラー: {e}")
}
