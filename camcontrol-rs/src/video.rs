//! Video files: writing (recording, time-lapse videos) and reading (playback,
//! thumbnails).
//!
//! - MP4: H.264 (OpenH264), small and plays almost anywhere.
//! - AVI: Motion JPEG, keeps more detail but much larger. Also reads the
//!   Python app's (OpenCV's) MJPG AVIs.
//! The Python app's MP4s use the older MPEG-4 Part 2 ("mp4v") codec, which
//! isn't supported here; they are reported as such.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::RgbImage;

/// Display name, file extension. MP4 is small and plays almost anywhere; AVI
/// (Motion JPEG) keeps more detail but is much larger.
pub const VIDEO_FORMATS: [(&str, &str); 2] = [("MP4", "mp4"), ("AVI (higher quality, large)", "avi")];
pub const VIDEO_EXTENSIONS: [&str; 2] = ["mp4", "avi"];

pub fn format_ext(fmt: &str) -> &'static str {
    VIDEO_FORMATS.iter().find(|f| f.0 == fmt).map(|f| f.1).unwrap_or("mp4")
}

pub fn is_video(path: &Path) -> bool {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

pub trait VideoWriter: Send {
    fn write(&mut self, frame: &RgbImage) -> Result<(), String>;
    /// Write the same frame again (cheap where the format allows).
    fn repeat(&mut self) -> Result<(), String>;
    fn finish(&mut self) -> Result<(), String>;
}

pub fn open_writer(path: &Path, fmt: &str, fps: f64, size: (u32, u32)) -> Result<Box<dyn VideoWriter>, String> {
    let w: Box<dyn VideoWriter> = match format_ext(fmt) {
        "avi" => Box::new(AviWriter::create(path, fps, size)?),
        _ => Box::new(Mp4Writer::create(path, fps, size)?),
    };
    Ok(w)
}

// --- JPEG ------------------------------------------------------------------------------

pub fn encode_jpeg(img: &RgbImage, quality: f32) -> Result<Vec<u8>, String> {
    std::panic::catch_unwind(|| -> std::io::Result<Vec<u8>> {
        let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        c.set_size(img.width() as usize, img.height() as usize);
        c.set_quality(quality);
        c.set_fastest_defaults();
        let mut started = c.start_compress(Vec::new())?;
        started.write_scanlines(img.as_raw())?;
        started.finish()
    })
    .map_err(|_| "JPEG encoder failed".to_string())?
    .map_err(|e| e.to_string())
}

pub fn decode_jpeg(data: &[u8]) -> Option<RgbImage> {
    image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).ok().map(|i| i.to_rgb8())
}

// --- AVI (Motion JPEG) -------------------------------------------------------------------

pub struct AviWriter {
    file: BufWriter<File>,
    size: (u32, u32),
    fps: f64,
    movi_start: u64, // position of the 'movi' fourcc
    index: Vec<(u32, u32)>, // offset from movi_start, size
    last: Option<Vec<u8>>,
}

impl AviWriter {
    pub fn create(path: &Path, fps: f64, size: (u32, u32)) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("Could not start a video at {}: {e}", path.display()))?;
        let mut w = Self { file: BufWriter::new(file), size, fps, movi_start: 0, index: Vec::new(), last: None };
        w.write_headers(0).map_err(|e| e.to_string())?;
        w.movi_start = w.file.stream_position().map_err(|e| e.to_string())? - 4;
        Ok(w)
    }

    /// RIFF header, hdrl list and the start of the movi list (sizes patched in finish()).
    fn write_headers(&mut self, frames: u32) -> std::io::Result<()> {
        let (w, h) = self.size;
        let f = &mut self.file;
        let scale = 1000u32;
        let rate = (self.fps * scale as f64).round() as u32;
        let le = |v: u32| v.to_le_bytes();
        f.write_all(b"RIFF")?;
        f.write_all(&le(0))?; // file size - 8, patched
        f.write_all(b"AVI LIST")?;
        f.write_all(&le(4 + 8 + 56 + 8 + 4 + 8 + 56 + 8 + 40))?;
        f.write_all(b"hdrlavih")?;
        f.write_all(&le(56))?;
        for v in [(1e6 / self.fps) as u32, 0, 0, 0x10 /* AVIF_HASINDEX */, frames, 0, 1, w * h * 3, w, h, 0, 0, 0, 0] {
            f.write_all(&le(v))?;
        }
        f.write_all(b"LIST")?;
        f.write_all(&le(4 + 8 + 56 + 8 + 40))?;
        f.write_all(b"strlstrh")?;
        f.write_all(&le(56))?;
        f.write_all(b"vidsMJPG")?;
        for v in [0, 0, 0, scale, rate, 0, frames, w * h * 3, u32::MAX, 0] {
            f.write_all(&le(v))?;
        }
        f.write_all(&[0u8; 8])?; // rcFrame
        f.write_all(b"strf")?;
        f.write_all(&le(40))?;
        for v in [40, w, h] {
            f.write_all(&le(v))?;
        }
        f.write_all(&1u16.to_le_bytes())?;
        f.write_all(&24u16.to_le_bytes())?;
        f.write_all(b"MJPG")?;
        for v in [w * h * 3, 0, 0, 0, 0] {
            f.write_all(&le(v))?;
        }
        f.write_all(b"LIST")?;
        f.write_all(&le(0))?; // movi size, patched
        f.write_all(b"movi")?;
        Ok(())
    }

    fn chunk(&mut self, data: &[u8]) -> Result<(), String> {
        let pos = self.file.stream_position().map_err(|e| e.to_string())?;
        if pos + data.len() as u64 > u32::MAX as u64 - 1_000_000 {
            return Err("The AVI file has reached its 4 GB limit.".into());
        }
        self.index.push(((pos - self.movi_start) as u32, data.len() as u32));
        let io = |r: std::io::Result<()>| r.map_err(|e| e.to_string());
        io(self.file.write_all(b"00dc"))?;
        io(self.file.write_all(&(data.len() as u32).to_le_bytes()))?;
        io(self.file.write_all(data))?;
        if data.len() % 2 == 1 {
            io(self.file.write_all(&[0]))?;
        }
        Ok(())
    }
}

impl VideoWriter for AviWriter {
    fn write(&mut self, frame: &RgbImage) -> Result<(), String> {
        let jpeg = encode_jpeg(frame, 90.0)?;
        self.chunk(&jpeg)?;
        self.last = Some(jpeg);
        Ok(())
    }

    fn repeat(&mut self) -> Result<(), String> {
        if let Some(j) = self.last.take() {
            self.chunk(&j)?;
            self.last = Some(j);
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        let io = |r: std::io::Result<()>| r.map_err(|e| e.to_string());
        let movi_end = self.file.stream_position().map_err(|e| e.to_string())?;
        io(self.file.write_all(b"idx1"))?;
        io(self.file.write_all(&((self.index.len() * 16) as u32).to_le_bytes()))?;
        for &(off, len) in &self.index.clone() {
            io(self.file.write_all(b"00dc"))?;
            io(self.file.write_all(&0x10u32.to_le_bytes()))?;
            io(self.file.write_all(&off.to_le_bytes()))?;
            io(self.file.write_all(&len.to_le_bytes()))?;
        }
        let end = self.file.stream_position().map_err(|e| e.to_string())?;
        let frames = self.index.len() as u32;
        io(self.file.seek(SeekFrom::Start(0)).map(|_| ()))?;
        io(self.write_headers(frames))?;
        io(self.file.seek(SeekFrom::Start(4)).map(|_| ()))?;
        io(self.file.write_all(&((end - 8) as u32).to_le_bytes()))?;
        io(self.file.seek(SeekFrom::Start(self.movi_start - 4)).map(|_| ()))?;
        io(self.file.write_all(&((movi_end - self.movi_start) as u32).to_le_bytes()))?;
        io(self.file.flush())
    }
}

// --- MP4 (H.264) -------------------------------------------------------------------------

pub struct Mp4Writer {
    encoder: openh264::encoder::Encoder,
    mp4: Option<mp4::Mp4Writer<BufWriter<File>>>,
    file: Option<BufWriter<File>>,
    size: (u32, u32), // even
    timescale: u32,
    frame_ticks: u32,
    frames: u64,
    yuv: Option<openh264::formats::YUVBuffer>,
}

fn nal_units(annex_b: &[u8]) -> Vec<&[u8]> {
    // Split on 00 00 01 / 00 00 00 01 start codes.
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= annex_b.len() {
        if annex_b[i] == 0 && annex_b[i + 1] == 0 && annex_b[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let mut e = if k + 1 < starts.len() { starts[k + 1] - 3 } else { annex_b.len() };
        while e > s && annex_b[e - 1] == 0 {
            e -= 1; // the 4-byte start code's leading zero
        }
        if e > s {
            out.push(&annex_b[s..e]);
        }
    }
    out
}

impl Mp4Writer {
    pub fn create(path: &Path, fps: f64, size: (u32, u32)) -> Result<Self, String> {
        use openh264::encoder::{BitRate, EncoderConfig, FrameRate, IntraFramePeriod};
        let size = (size.0 & !1, size.1 & !1); // 4:2:0 needs even sizes
        let fps_i = fps.round().max(1.0) as u32;
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(((size.0 * size.1) as f64 * fps * 0.15).clamp(500_000.0, 40_000_000.0) as u32))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps_i));
        let encoder = openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(|e| format!("Could not start the video encoder: {e}"))?;
        let file = File::create(path).map_err(|e| format!("Could not start a video at {}: {e}", path.display()))?;
        let timescale = 1000 * fps_i;
        Ok(Self {
            encoder,
            mp4: None,
            file: Some(BufWriter::new(file)),
            size,
            timescale,
            frame_ticks: (timescale as f64 / fps).round() as u32,
            frames: 0,
            yuv: None,
        })
    }

    fn encode(&mut self) -> Result<(), String> {
        let yuv = self.yuv.as_ref().unwrap();
        let bits = self.encoder.encode(yuv).map_err(|e| e.to_string())?.to_vec();
        let nals = nal_units(&bits);
        if self.mp4.is_none() {
            let sps = nals.iter().find(|n| n[0] & 0x1f == 7).ok_or("no SPS from the encoder")?;
            let pps = nals.iter().find(|n| n[0] & 0x1f == 8).ok_or("no PPS from the encoder")?;
            let config = mp4::Mp4Config {
                major_brand: str::parse("isom").unwrap(),
                minor_version: 512,
                compatible_brands: vec![str::parse("isom").unwrap(), str::parse("iso2").unwrap(),
                                        str::parse("avc1").unwrap(), str::parse("mp41").unwrap()],
                timescale: self.timescale,
            };
            let mut w = mp4::Mp4Writer::write_start(self.file.take().unwrap(), &config).map_err(|e| e.to_string())?;
            w.add_track(&mp4::TrackConfig {
                track_type: mp4::TrackType::Video,
                timescale: self.timescale,
                language: "und".into(),
                media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                    width: self.size.0 as u16,
                    height: self.size.1 as u16,
                    seq_param_set: sps.to_vec(),
                    pic_param_set: pps.to_vec(),
                }),
            }).map_err(|e| e.to_string())?;
            self.mp4 = Some(w);
        }
        let mut sample = Vec::new();
        let mut sync = false;
        for n in nals.iter().filter(|n| !matches!(n[0] & 0x1f, 7 | 8 | 9)) {
            sync |= n[0] & 0x1f == 5;
            sample.extend_from_slice(&(n.len() as u32).to_be_bytes());
            sample.extend_from_slice(n);
        }
        if sample.is_empty() {
            return Ok(()); // the encoder skipped this frame
        }
        let s = mp4::Mp4Sample {
            start_time: self.frames * self.frame_ticks as u64,
            duration: self.frame_ticks,
            rendering_offset: 0,
            is_sync: sync,
            bytes: mp4::Bytes::from(sample),
        };
        self.mp4.as_mut().unwrap().write_sample(1, &s).map_err(|e| e.to_string())?;
        self.frames += 1;
        Ok(())
    }
}

impl VideoWriter for Mp4Writer {
    fn write(&mut self, frame: &RgbImage) -> Result<(), String> {
        let (w, h) = self.size;
        let frame = if frame.dimensions() != (w, h) {
            image::imageops::crop_imm(frame, 0, 0, w.min(frame.width()), h.min(frame.height())).to_image()
        } else {
            frame.clone()
        };
        if frame.dimensions() != (w, h) {
            return Err("frame size changed during the recording".into());
        }
        let src = openh264::formats::RgbSliceU8::new(frame.as_raw(), (w as usize, h as usize));
        self.yuv = Some(openh264::formats::YUVBuffer::from_rgb8_source(src));
        self.encode()
    }

    fn repeat(&mut self) -> Result<(), String> {
        if self.yuv.is_some() { self.encode() } else { Ok(()) }
    }

    fn finish(&mut self) -> Result<(), String> {
        if let Some(mut w) = self.mp4.take() {
            w.write_end().map_err(|e| e.to_string())?;
            w.into_writer().flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

// --- reading ------------------------------------------------------------------------------

enum Source {
    Avi { file: File, frames: Vec<(u64, u32)> },
    Mp4 {
        reader: mp4::Mp4Reader<std::io::BufReader<File>>,
        track: u32,
        decoder: openh264::decoder::Decoder,
        sps: Vec<u8>,
        pps: Vec<u8>,
        next: u32, // next sample the decoder expects (1-based)
    },
}

pub struct VideoReader {
    pub path: PathBuf,
    pub fps: f64,
    pub frame_count: usize,
    source: Source,
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

impl VideoReader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let mut head = [0u8; 12];
        file.read_exact(&mut head).map_err(|_| "Not a video file.".to_string())?;
        if &head[0..4] == b"RIFF" && &head[8..12] == b"AVI " {
            return Self::open_avi(path, file);
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        Self::open_mp4(path, file)
    }

    fn open_avi(path: &Path, mut file: File) -> Result<Self, String> {
        let mut data = Vec::new();
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let (mut fps, mut frames) = (25.0, Vec::new());
        // Walk the chunks: strh gives the rate, movi holds the frames.
        fn walk(d: &[u8], mut i: usize, end: usize, fps: &mut f64, frames: &mut Vec<(u64, u32)>, base: usize) {
            while i + 8 <= end {
                let id = &d[i..i + 4];
                let size = read_u32(d, i + 4) as usize;
                let body = i + 8;
                if body + size > d.len() {
                    break;
                }
                if id == b"LIST" || id == b"RIFF" {
                    walk(d, body + 4, body + size, fps, frames, base);
                } else if id == b"strh" && &d[body..body + 4] == b"vids" {
                    let (scale, rate) = (read_u32(d, body + 20), read_u32(d, body + 24));
                    if scale > 0 && rate > 0 {
                        *fps = rate as f64 / scale as f64;
                    }
                } else if id.len() == 4 && (&id[2..4] == b"dc" || &id[2..4] == b"db") && size > 0 {
                    frames.push(((body + base) as u64, size as u32));
                }
                i = body + size + (size & 1);
            }
        }
        walk(&data, 12, data.len(), &mut fps, &mut frames, 0);
        if frames.is_empty() {
            return Err("No frames in this AVI.".into());
        }
        let fps = if (1.0..=240.0).contains(&fps) { fps } else { 25.0 };
        Ok(Self { path: path.into(), fps, frame_count: frames.len(), source: Source::Avi { file, frames } })
    }

    fn open_mp4(path: &Path, file: File) -> Result<Self, String> {
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        let reader = mp4::Mp4Reader::read_header(std::io::BufReader::new(file), size).map_err(|e| format!("Unreadable video: {e}"))?;
        let (track, t) = reader
            .tracks()
            .iter()
            .find(|(_, t)| matches!(t.track_type(), Ok(mp4::TrackType::Video)))
            .map(|(id, t)| (*id, t))
            .ok_or("No video in this file.")?;
        match t.media_type() {
            Ok(mp4::MediaType::H264) => {}
            _ => return Err("This video uses a codec this version can't play (e.g. MPEG-4 Part 2 'mp4v', as the \
                             Python CamControl recorded). Videos recorded here are H.264 MP4 or MJPEG AVI.".into()),
        }
        let sps = t.sequence_parameter_set().map_err(|e| e.to_string())?.to_vec();
        let pps = t.picture_parameter_set().map_err(|e| e.to_string())?.to_vec();
        let fps = t.frame_rate();
        let fps = if (1.0..=240.0).contains(&fps) { fps } else { 25.0 };
        let frame_count = t.sample_count() as usize;
        let decoder = openh264::decoder::Decoder::new().map_err(|e| e.to_string())?;
        Ok(Self { path: path.into(), fps, frame_count, source: Source::Mp4 { reader, track, decoder, sps, pps, next: 1 } })
    }

    /// Frame `index` (0-based), or None past the end / if unreadable.
    pub fn frame(&mut self, index: usize) -> Option<RgbImage> {
        if index >= self.frame_count {
            return None;
        }
        match &mut self.source {
            Source::Avi { file, frames } => {
                let (off, len) = frames[index];
                let mut buf = vec![0u8; len as usize];
                file.seek(SeekFrom::Start(off)).ok()?;
                file.read_exact(&mut buf).ok()?;
                decode_jpeg(&buf)
            }
            Source::Mp4 { reader, track, decoder, sps, pps, next } => {
                let target = index as u32 + 1;
                if target != *next {
                    // Back to the nearest sync sample at or before the target.
                    let mut k = target;
                    while k > 1 {
                        match reader.read_sample(*track, k) {
                            Ok(Some(s)) if s.is_sync => break,
                            _ => k -= 1,
                        }
                    }
                    *decoder = openh264::decoder::Decoder::new().ok()?;
                    *next = k;
                }
                let mut out = None;
                while *next <= target {
                    let s = reader.read_sample(*track, *next).ok()??;
                    let mut annex = Vec::new();
                    if s.is_sync {
                        for ps in [&*sps, &*pps] {
                            annex.extend_from_slice(&[0, 0, 0, 1]);
                            annex.extend_from_slice(ps);
                        }
                    }
                    let b = &s.bytes;
                    let mut i = 0;
                    while i + 4 <= b.len() {
                        let n = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
                        annex.extend_from_slice(&[0, 0, 0, 1]);
                        annex.extend_from_slice(&b[i + 4..(i + 4 + n).min(b.len())]);
                        i += 4 + n;
                    }
                    *next += 1;
                    let decoded = decoder.decode(&annex).ok()?;
                    if *next > target {
                        if let Some(yuv) = decoded {
                            use openh264::formats::YUVSource;
                            let (w, h) = yuv.dimensions();
                            let mut rgb = vec![0u8; w * h * 3];
                            yuv.write_rgb8(&mut rgb);
                            out = RgbImage::from_raw(w as u32, h as u32, rgb);
                        }
                    }
                }
                out
            }
        }
    }
}

/// The first frame of a video (for thumbnails).
pub fn first_frame(path: &Path) -> Option<RgbImage> {
    VideoReader::open(path).ok()?.frame(0)
}

// --- recording -----------------------------------------------------------------------------

/// Writes live frames to a video file in real time: each frame fills every
/// output slot that has passed since the start, so the video plays at real
/// speed whatever the camera manages.
pub struct VideoRecorder {
    pub path: PathBuf,
    pub fmt: String,
    pub fps: f64,
    pub max_seconds: f64,
    pub frames_written: u64,
    start: Option<Instant>,
    writer: Option<Box<dyn VideoWriter>>,
}

impl VideoRecorder {
    pub fn new(path: PathBuf, fmt: &str, fps: f64, max_seconds: f64) -> Self {
        Self { path, fmt: fmt.into(), fps, max_seconds, frames_written: 0, start: None, writer: None }
    }

    pub fn elapsed(&self) -> f64 {
        self.start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    pub fn finished(&self) -> bool {
        self.max_seconds > 0.0 && self.elapsed() >= self.max_seconds
    }

    pub fn add(&mut self, frame: &RgbImage) -> Result<(), String> {
        if self.writer.is_none() {
            self.writer = Some(open_writer(&self.path, &self.fmt, self.fps, frame.dimensions())?);
            self.start = Some(Instant::now());
        }
        let mut due = (self.elapsed() * self.fps) as u64 + 1;
        if self.max_seconds > 0.0 {
            due = due.min((self.max_seconds * self.fps).round() as u64);
        }
        let w = self.writer.as_mut().unwrap();
        let mut first = true;
        while self.frames_written < due {
            if first { w.write(frame)? } else { w.repeat()? }
            first = false;
            self.frames_written += 1;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), String> {
        match self.writer.take() {
            Some(mut w) => w.finish(),
            None => Ok(()),
        }
    }
}

/// Join images into a video, all at the first image's size.
pub fn images_to_video(paths: &[PathBuf], out: &Path, fmt: &str, fps: f64) -> Result<PathBuf, String> {
    let mut writer: Option<Box<dyn VideoWriter>> = None;
    let mut size = (0, 0);
    for p in paths {
        let Some(img) = crate::capture::load_image(p) else { continue };
        let img = if writer.is_none() {
            size = img.dimensions();
            writer = Some(open_writer(out, fmt, fps, size)?);
            img
        } else if img.dimensions() != size {
            crate::imgproc::resize_rgb(&img, size.0, size.1, crate::imgproc::Interp::Area)
        } else {
            img
        };
        writer.as_mut().unwrap().write(&img)?;
    }
    let mut w = writer.ok_or("No readable images to make a video from.")?;
    w.finish()?;
    Ok(out.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn frame(i: u32) -> RgbImage {
        RgbImage::from_fn(160, 120, |x, y| Rgb([((x + i * 20) % 255) as u8, (y * 2) as u8, (i * 40 % 255) as u8]))
    }

    #[test]
    fn avi_and_mp4_round_trip() {
        let dir = std::env::temp_dir().join(format!("camcontrol-rs-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (fmt, name) in [("AVI (higher quality, large)", "t.avi"), ("MP4", "t.mp4")] {
            let path = dir.join(name);
            let mut w = open_writer(&path, fmt, 20.0, (160, 120)).unwrap();
            for i in 0..12 {
                w.write(&frame(i)).unwrap();
            }
            w.repeat().unwrap();
            w.finish().unwrap();
            let mut r = VideoReader::open(&path).unwrap();
            assert_eq!(r.frame_count, 13, "{fmt}");
            assert!((r.fps - 20.0).abs() < 0.01, "{fmt} fps {}", r.fps);
            let f5 = r.frame(5).unwrap();
            assert_eq!(f5.dimensions(), (160, 120));
            let want = frame(5);
            let err: f64 = f5.pixels().zip(want.pixels()).map(|(a, b)| (a.0[0] as f64 - b.0[0] as f64).abs()).sum::<f64>() / (160.0 * 120.0);
            assert!(err < 20.0, "{fmt} frame 5 error {err}");
            // Seeking back works.
            assert!(r.frame(1).is_some() && r.frame(12).is_some());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recorder_fills_slots() {
        let dir = std::env::temp_dir().join(format!("camcontrol-rs-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rec = VideoRecorder::new(dir.join("r.avi"), "AVI (higher quality, large)", 20.0, 0.0);
        let t = Instant::now();
        while t.elapsed().as_secs_f64() < 0.5 {
            rec.add(&frame(rec.frames_written as u32)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        rec.close().unwrap();
        assert!((9..=13).contains(&rec.frames_written), "{}", rec.frames_written);
        assert_eq!(VideoReader::open(&dir.join("r.avi")).unwrap().frame_count as u64, rec.frames_written);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
