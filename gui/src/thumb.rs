//! Background thumbnail + EXIF-summary worker (plan 11 §6): one dedicated
//! decode thread fed through a `std::sync::mpsc` channel, **paused while
//! a job runs**, with a bounded decode (`image::io::Limits`, 512 MiB
//! allocation cap) and an LRU texture cache (cap 1024 entries ≈ ≤ 40 MiB
//! of 96 px RGBA textures; key `(path, mtime)` — re-conversion invalidates
//! via the newer mtime).
//!
//! Requests come from the file table's visible rows only (the
//! virtualized `body.rows` callback yields the visible index range each
//! frame); the UI thread drains results each frame like job events.
//! Decoding never touches the UI thread; undecodable files produce a
//! `Failed` placeholder marker, not an error path.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, SystemTime};

use byteshaver::metadata::exif::ExifSummary;
use egui::TextureHandle;
use image::RgbaImage;

/// Longest edge of a generated thumbnail texture (2× the 48 px cell for
/// HiDPI rendering).
pub const THUMB_EDGE: u32 = 96;
/// Decode allocation cap (plan 11 §6 overhead budget).
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
/// LRU cache cap: 1024 × (96² × 4 B) ≈ 37 MiB of texture memory.
const CACHE_CAP: usize = 1024;
/// Worker poll interval while paused / idle.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// When thumbnails are shown (persisted setting; plan 11 §6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThumbMode {
    /// Never (column space stays at the compact row height).
    Off,
    /// For every queued file row from enqueue on.
    OnAdd,
    /// Once a row entered a conversion run (default per plan 11 §6).
    #[default]
    OnConvert,
}

/// Cache key: enqueued path plus the enqueue-time mtime.
pub type ThumbKey = (PathBuf, Option<SystemTime>);

/// Cached thumbnail state for one key.
pub enum ThumbEntry {
    /// Decoded texture (loaded on the UI thread from the worker's RGBA8).
    Ready(TextureHandle),
    /// Undecodable/unsupported — render the placeholder glyph.
    Failed,
}

/// Work item handed to the decode thread.
pub enum WorkerJob {
    /// Decode + downscale one image.
    Thumbnail { key: ThumbKey },
    /// Read the typed EXIF summary of one file.
    Exif { path: PathBuf },
}

/// Result produced by the decode thread.
pub enum WorkerResult {
    /// `None` = decode failed (placeholder).
    Thumbnail {
        key: ThumbKey,
        image: Option<RgbaImage>,
    },
    /// Summary read outcome (`None` = no EXIF/unparsable).
    Exif {
        path: PathBuf,
        summary: Option<ExifSummary>,
    },
}

/// Minimal LRU cache (pure, unit-testable): promotion on `get`, oldest
/// entry evicted past the cap.
pub struct LruCache<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    cap: usize,
}

impl<K: Clone + Eq + std::hash::Hash, V> LruCache<K, V> {
    /// Creates an empty cache with the given entry cap.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        LruCache {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Returns the cached value, promoting it to most-recently-used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            self.promote(key);
        }
        self.map.get(key)
    }

    /// Whether `key` is cached (without promotion).
    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    /// Inserts/updates `key`, evicting the least-recently-used entry past
    /// the cap.
    pub fn insert(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            self.promote(&key);
        } else {
            self.order.push_back(key.clone());
        }
        self.map.insert(key, value);
        while self.map.len() > self.cap {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&evicted);
        }
    }

    fn promote(&mut self, key: &K) {
        if let Some(position) = self.order.iter().position(|entry| entry == key) {
            self.order.remove(position);
        }
        self.order.push_back(key.clone());
    }
}

/// UI-side handle of the thumbnail/EXIF worker: request dedup, result
/// draining and the texture cache. Dropping it shuts the worker down.
pub struct ThumbState {
    jobs: Sender<WorkerJob>,
    results: Receiver<WorkerResult>,
    paused: Arc<AtomicBool>,
    /// Paths with an in-flight thumbnail request (dedup).
    pending_thumbs: HashSet<PathBuf>,
    /// Paths with an in-flight EXIF request (dedup).
    pending_exif: HashSet<PathBuf>,
    /// Decoded textures / failure markers, LRU-capped.
    cache: LruCache<ThumbKey, ThumbEntry>,
}

impl Default for ThumbState {
    fn default() -> Self {
        Self::new()
    }
}

impl ThumbState {
    /// Spawns the decode thread (once per app).
    #[must_use]
    pub fn new() -> Self {
        let (jobs, job_rx) = mpsc::channel::<WorkerJob>();
        let (result_tx, results) = mpsc::channel::<WorkerResult>();
        let paused = Arc::new(AtomicBool::new(false));
        std::thread::Builder::new()
            .name("byteshaver-thumbs".to_string())
            .spawn({
                let paused = Arc::clone(&paused);
                move || worker_loop(job_rx, result_tx, paused)
            })
            .expect("spawn thumbnail worker");
        ThumbState {
            jobs,
            results,
            paused,
            pending_thumbs: HashSet::new(),
            pending_exif: HashSet::new(),
            cache: LruCache::new(CACHE_CAP),
        }
    }

    /// Pause flag toggled by the app each frame (decoding contends with a
    /// saturating conversion job for no user-visible benefit).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// (Re-)requests thumbnails, most-recently-visible-first (the table
    /// passes its visible rows bottom-up). Requests for cached or
    /// in-flight paths are dropped.
    pub fn request_thumbs(&mut self, keys: &[ThumbKey]) {
        for key in keys.iter().rev() {
            let (path, mtime) = key;
            if self.cache.contains(&(path.clone(), *mtime)) || self.pending_thumbs.contains(path) {
                continue;
            }
            if self
                .jobs
                .send(WorkerJob::Thumbnail { key: key.clone() })
                .is_err()
            {
                return;
            }
            self.pending_thumbs.insert(path.clone());
        }
    }

    /// Requests the EXIF summary of `path` (deduped; the result lands on
    /// the queue row via the app's drain callback).
    pub fn request_exif(&mut self, path: PathBuf) {
        if self.pending_exif.contains(&path) {
            return;
        }
        if self
            .jobs
            .send(WorkerJob::Exif { path: path.clone() })
            .is_err()
        {
            return;
        }
        self.pending_exif.insert(path);
    }

    /// Cached entry for `key` (promotes it).
    pub fn cached(&mut self, key: &ThumbKey) -> Option<&ThumbEntry> {
        self.cache.get(key)
    }

    /// Drains the worker results: textures land in the LRU cache, EXIF
    /// summaries are handed to `apply_exif` (the app writes them onto the
    /// queue rows).
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        mut apply_exif: impl FnMut(PathBuf, Option<ExifSummary>),
    ) {
        loop {
            match self.results.try_recv() {
                Ok(WorkerResult::Thumbnail { key, image }) => {
                    self.pending_thumbs.remove(&key.0);
                    match image {
                        Some(image) => {
                            let name = format!("thumb-{}", key.0.display());
                            let size = [image.width() as usize, image.height() as usize];
                            let color =
                                egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                            let texture =
                                ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
                            self.cache.insert(key, ThumbEntry::Ready(texture));
                        }
                        None => {
                            self.cache.insert(key, ThumbEntry::Failed);
                        }
                    }
                }
                Ok(WorkerResult::Exif { path, summary }) => {
                    self.pending_exif.remove(&path);
                    apply_exif(path, summary);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }
}
/// Decode-thread loop: sleeps while paused, otherwise takes jobs with a
/// timeout; exits when all senders are gone (the app was dropped).
fn worker_loop(jobs: Receiver<WorkerJob>, results: Sender<WorkerResult>, paused: Arc<AtomicBool>) {
    loop {
        if paused.load(Ordering::Relaxed) {
            std::thread::sleep(POLL_INTERVAL);
            continue;
        }
        match jobs.recv_timeout(POLL_INTERVAL) {
            Ok(job) => {
                // decode right away: pausing only between items keeps a
                // just-picked job from being lost (requests are deduped by
                // the pending set, so a dropped job would never be retried)
                let result = match job {
                    WorkerJob::Thumbnail { key } => WorkerResult::Thumbnail {
                        image: decode_thumbnail(&key.0),
                        key,
                    },
                    WorkerJob::Exif { path } => WorkerResult::Exif {
                        summary: byteshaver::metadata::exif::read_summary(&path),
                        path,
                    },
                };
                if results.send(result).is_err() {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Bounded decode + downscale to [`THUMB_EDGE`] (never on the UI thread).
/// `None` on any failure (missing file, unsupported/corrupt format,
/// allocation-limit hit).
#[must_use]
fn decode_thumbnail(path: &Path) -> Option<RgbaImage> {
    let mut reader = image::ImageReader::open(path).ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let reader = reader.with_guessed_format().ok()?;
    let image = reader.decode().ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = fit_inside(rgba.width(), rgba.height(), THUMB_EDGE);
    Some(image::imageops::thumbnail(
        &rgba,
        width.max(1),
        height.max(1),
    ))
}

/// Scales `(width, height)` to fit inside `edge` (aspect preserved, no
/// upscaling; zero inputs stay zero → clamped by the caller).
#[must_use]
pub fn fit_inside(width: u32, height: u32, edge: u32) -> (u32, u32) {
    if width == 0 || height == 0 || (width <= edge && height <= edge) {
        return (width, height);
    }
    if width >= height {
        (
            edge,
            ((u64::from(height) * u64::from(edge)) / u64::from(width)).max(1) as u32,
        )
    } else {
        (
            ((u64::from(width) * u64::from(edge)) / u64::from(height)).max(1) as u32,
            edge,
        )
    }
}

/// Header-only dimension read for the Dimensions column (~µs, no decode).
#[must_use]
pub fn read_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut reader = image::ImageReader::open(path).ok()?;
    reader.limits({
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODE_ALLOC);
        limits
    });
    let reader = reader.with_guessed_format().ok()?;
    reader.into_dimensions().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- LRU cache ----------------------------------------------------------

    #[test]
    fn lru_evicts_least_recently_used_past_the_cap() {
        let mut cache = LruCache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.get(&"a"), Some(&1), "promote a");
        cache.insert("c", 3); // evicts b (a was promoted)
        assert!(!cache.contains(&"b"));
        assert!(cache.contains(&"a") && cache.contains(&"c"));

        cache.insert("d", 4); // evicts a (least recently used now)
        assert!(!cache.contains(&"a"));
        assert_eq!(cache.get(&"c"), Some(&3));
        assert!(cache.contains(&"d"));
    }

    #[test]
    fn cache_key_includes_mtime_so_reconversion_invalidates() {
        let mut cache: LruCache<ThumbKey, u32> = LruCache::new(CACHE_CAP);
        let path = PathBuf::from("/x/a.png");
        let old = SystemTime::UNIX_EPOCH;
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        cache.insert((path.clone(), Some(old)), 1);
        // after a re-encode the row carries a newer mtime → cache miss →
        // the worker re-decodes and inserts a fresh entry
        assert!(cache.contains(&(path.clone(), Some(old))));
        assert!(!cache.contains(&(path.clone(), Some(new))));
        cache.insert((path.clone(), Some(new)), 2);
        assert_eq!(cache.get(&(path.clone(), Some(new))), Some(&2));
        // missing-mtime rows (unstatable at enqueue) get their own slot
        cache.insert((path, None), 3);
        assert!(cache.contains(&(PathBuf::from("/x/a.png"), None)));
        assert!(cache.contains(&(PathBuf::from("/x/a.png"), Some(old))));
    }

    // ---- scaling ------------------------------------------------------------

    #[test]
    fn fit_inside_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_inside(4000, 3000, 96), (96, 72));
        assert_eq!(fit_inside(3000, 4000, 96), (72, 96));
        assert_eq!(fit_inside(96, 96, 96), (96, 96));
        assert_eq!(fit_inside(50, 20, 96), (50, 20));
        // degenerate inputs stay degenerate (caller clamps to 1)
        assert_eq!(fit_inside(0, 10, 96), (0, 10));
        // extreme aspect ratios never produce a zero edge
        let (w, h) = fit_inside(10_000, 3, 96);
        assert_eq!((w, h), (96, 1));
        let (w, h) = fit_inside(3, 10_000, 96);
        assert_eq!((w, h), (1, 96));
    }

    // ---- decode pipeline ------------------------------------------------------

    /// Decode of a real (generated) PNG round-trips through the helpers;
    /// undecodable files yield the failure marker instead of an error.
    #[test]
    fn decode_pipeline_reports_failures_as_placeholders() {
        let buffer = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(buffer)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode fixture");
        let dir = std::env::temp_dir().join(format!("byteshaver-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let good = dir.join("good.png");
        std::fs::write(&good, &png).expect("write png");
        let bad = dir.join("bad.png");
        std::fs::write(&bad, b"not an image").expect("write junk");

        assert_eq!(
            read_dimensions(&good),
            Some((2, 2)),
            "header-only read needs no decode"
        );
        assert_eq!(read_dimensions(&bad), None);
        assert_eq!(read_dimensions(&dir.join("missing.png")), None);

        let thumb = decode_thumbnail(&good).expect("decodes");
        assert_eq!((thumb.width(), thumb.height()), (2, 2), "no upscaling");
        assert_eq!(decode_thumbnail(&bad), None, "failure → placeholder");
        assert_eq!(decode_thumbnail(&dir.join("missing.png")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
