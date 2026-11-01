//! Pictures through kitty graphics with Unicode placeholders, the subset Claude Code uses.
//!
//! Claude Code sends a PNG once (`ESC _ G a=T,U=1,i=<id>,c=<columns>,r=<rows>;<base64> ESC \`,
//! chunked with `m=1` ... `m=0`), then writes ordinary grid cells: `PLACEHOLDER`, foreground
//! color = image id, first combining mark = image row, second = image column. The grid holds
//! the picture like text, so scroll, resize, clear and scrollback need nothing here.
//!
//! alacritty's parser drops APC strings, so `GraphicsPty` takes them out of the PTY stream
//! before it: every other byte passes through unchanged.
//!
//! On this machine cc-intelligent-ui gives Claude Code PNG files, and Claude Code sends their
//! path (`t=f`) once the terminal says it reads files; bytes (`t=d`) are for ssh.

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    fs::{self, File},
    io::{self, Cursor, Read},
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use alacritty_terminal::{
    event::{Event, EventListener, OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite, Pty},
    vte::ansi::Color,
};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use gpui::RenderImage;
use image::{DynamicImage, Frame, ImageFormat, ImageReader, Limits, RgbImage, RgbaImage};

/// The cell char that stands for a slice of a picture.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Decoded pictures one tab keeps; past it the oldest go.
const MAX_TAB_BYTES: usize = 256 << 20;
/// Largest picture side accepted (Claude Code sends at most 4096).
const MAX_SIDE: u32 = 8192;
/// Largest base64 transmission accepted (Claude Code sends at most 2 MiB of PNG).
const MAX_ENCODED: usize = 16 << 20;
/// Largest picture file read for `t=f` (a 4096 x 4096 RGBA file is 64 MiB).
const MAX_FILE: u64 = 64 << 20;
const ESC: u8 = 0x1b;

/// kitty's `rowcolumn-diacritics.txt`: the mark at index `n` means row or column `n`.
#[rustfmt::skip]
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F,
    0x0346, 0x034A, 0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357,
    0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484,
    0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
    0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611,
    0x0612, 0x0613, 0x0614, 0x0615, 0x0616, 0x0617, 0x0657, 0x0658,
    0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
    0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733,
    0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE,
    0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817, 0x0818, 0x0819,
    0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C,
    0x082D, 0x0951, 0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87,
    0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D,
    0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
    0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1,
    0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5, 0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9,
    0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
    0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7,
    0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA,
    0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2,
    0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D,
    0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5,
    0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7,
    0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
    0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187,
    0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243,
    0x1D244,
];

/// Row or column a placeholder's combining mark stands for.
fn diacritic_index(mark: char) -> Option<u32> {
    DIACRITICS
        .binary_search(&(mark as u32))
        .ok()
        .map(|index| index as u32)
}

/// One slice of a picture, read from a placeholder cell: image id, image row, image column.
pub fn placeholder(fg: Color, marks: &[char]) -> Option<(u32, u32, u32)> {
    let id = match fg {
        Color::Indexed(index) => u32::from(index),
        Color::Spec(rgb) => u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b),
        Color::Named(_) => return None,
    };
    let [row, column, ..] = marks else {
        return None;
    };
    Some((id, diacritic_index(*row)?, diacritic_index(*column)?))
}

/// A decoded picture and the cell box it was sent for.
pub struct Picture {
    pub image: Arc<RenderImage>,
    /// Box in cells (`c`, `r`); 0 when the sender gave none.
    pub columns: u32,
    pub rows: u32,
}

#[derive(Default)]
struct Store {
    pictures: HashMap<u32, Arc<Picture>>,
    /// Ids, oldest first.
    order: VecDeque<u32>,
    bytes: usize,
    /// Replaced or dropped pictures whose GPU textures the view still has to free.
    retired: Vec<Arc<RenderImage>>,
}

impl Store {
    fn remove(&mut self, id: u32) {
        if let Some(picture) = self.pictures.remove(&id) {
            self.order.retain(|&known| known != id);
            self.bytes -= size_of(&picture.image);
            self.retired.push(picture.image.clone());
        }
    }
}

fn size_of(image: &RenderImage) -> usize {
    image.as_bytes(0).map_or(0, <[u8]>::len)
}

/// One tab's pictures, shared by the PTY side (writes) and the view (reads at paint).
#[derive(Clone, Default)]
pub struct Images(Arc<Mutex<Store>>);

impl Images {
    pub fn get(&self, id: u32) -> Option<Arc<Picture>> {
        self.0.lock().ok()?.pictures.get(&id).cloned()
    }

    /// Textures to free: replaced and dropped pictures, plus every picture when `all`.
    pub fn take_retired(&self, all: bool) -> Vec<Arc<RenderImage>> {
        let Ok(mut store) = self.0.lock() else {
            return Vec::new();
        };
        if all {
            let ids: Vec<u32> = store.order.iter().copied().collect();
            for id in ids {
                store.remove(id);
            }
        }
        std::mem::take(&mut store.retired)
    }

    fn insert(&self, id: u32, picture: Picture) {
        let Ok(mut store) = self.0.lock() else {
            return;
        };
        store.remove(id);
        store.bytes += size_of(&picture.image);
        store.order.push_back(id);
        store.pictures.insert(id, Arc::new(picture));
        while store.bytes > MAX_TAB_BYTES && store.order.len() > 1 {
            let Some(oldest) = store.order.front().copied() else {
                break;
            };
            log::info!("graphics: over {MAX_TAB_BYTES} bytes, dropping picture {oldest}");
            store.remove(oldest);
        }
    }

    fn remove(&self, id: Option<u32>) {
        let Ok(mut store) = self.0.lock() else {
            return;
        };
        match id {
            Some(id) => store.remove(id),
            None => {
                let ids: Vec<u32> = store.order.iter().copied().collect();
                for id in ids {
                    store.remove(id);
                }
            }
        }
    }
}

/// How a picture's bytes arrive and what they are.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Format {
    /// `t=f`: the payload is the path of a file holding the bytes.
    file: bool,
    /// `f`: 100 PNG, 32 RGBA, 24 RGB.
    pixels: u32,
    /// `s`, `v`: size of raw RGB(A) pixels.
    width: u32,
    height: u32,
}

/// Work for a tab's decoder thread, in arrival order, so a delete can't overtake its picture.
enum Job {
    Decode {
        id: u32,
        base64: Vec<u8>,
        format: Format,
        columns: u32,
        rows: u32,
    },
    /// One picture, or all (`None`).
    Delete(Option<u32>),
}

/// Starts a tab's decoder thread; it ends when the sender (the tab's PTY) is dropped.
fn decoder(images: Images, listener: Arc<dyn EventListener + Send + Sync>) -> mpsc::Sender<Job> {
    let (jobs, inbox) = mpsc::channel();
    thread::Builder::new()
        .name("graphics".into())
        .spawn(move || {
            for job in inbox {
                match job {
                    Job::Decode {
                        id,
                        base64,
                        format,
                        columns,
                        rows,
                    } => match decode(&base64, format) {
                        Ok(image) => {
                            let pixels = image.size(0);
                            log::info!(
                                "graphics: picture {id} {}x{} px in {columns}x{rows} cells",
                                pixels.width.0,
                                pixels.height.0
                            );
                            images.insert(
                                id,
                                Picture {
                                    image: Arc::new(image),
                                    columns,
                                    rows,
                                },
                            );
                            listener.send_event(Event::Wakeup);
                        }
                        Err(error) => log::warn!("graphics: picture {id} dropped: {error}"),
                    },
                    Job::Delete(id) => {
                        images.remove(id);
                        listener.send_event(Event::Wakeup);
                    }
                }
            }
        })
        .map_err(|error| log::warn!("graphics: no decoder thread: {error}"))
        .ok();
    jobs
}

fn unbase64(base64: &[u8]) -> Result<Vec<u8>, String> {
    let engine = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    engine
        .decode(base64)
        .map_err(|error| format!("bad base64: {error}"))
}

/// The file a `t=f` command names (its payload): a regular file of at most `MAX_FILE` bytes.
fn open_file(base64: &[u8]) -> Result<File, String> {
    let path = unbase64(base64)?;
    let path = Path::new(OsStr::from_bytes(&path));
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE {
        return Err(format!(
            "{}: not a file under {MAX_FILE} bytes",
            path.display()
        ));
    }
    File::open(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn decode(base64: &[u8], format: Format) -> Result<RenderImage, String> {
    let bytes = if format.file {
        let mut bytes = Vec::new();
        open_file(base64)?
            .take(MAX_FILE)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("can't read the file: {error}"))?;
        bytes
    } else {
        unbase64(base64)?
    };
    let (width, height) = (format.width, format.height);
    let raw_size = |channels: u32| {
        let fits = (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height);
        let size = (width * height * channels) as usize;
        (fits && bytes.len() >= size).then_some(size).ok_or(format!(
            "{width}x{height} pixels don't match {} bytes",
            bytes.len()
        ))
    };
    let mut pixels = match format.pixels {
        100 => {
            let mut limits = Limits::default();
            limits.max_image_width = Some(MAX_SIDE);
            limits.max_image_height = Some(MAX_SIDE);
            let mut reader = ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Png);
            reader.limits(limits);
            reader
                .decode()
                .map_err(|error| format!("bad PNG: {error}"))?
                .into_rgba8()
        }
        32 => {
            let size = raw_size(4)?;
            RgbaImage::from_raw(width, height, bytes[..size].to_vec()).ok_or("bad RGBA")?
        }
        24 => {
            let size = raw_size(3)?;
            let rgb = RgbImage::from_raw(width, height, bytes[..size].to_vec()).ok_or("bad RGB")?;
            DynamicImage::ImageRgb8(rgb).into_rgba8()
        }
        other => return Err(format!("format f={other} not supported")),
    };
    // GPUI's textures are BGRA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(RenderImage::new(vec![Frame::new(pixels)]))
}

/// Where the filter is in the byte stream.
#[derive(Clone, Copy, PartialEq)]
enum State {
    Ground,
    /// Saw ESC, held back.
    Esc,
    /// Saw ESC _, held back.
    Apc,
    /// Inside ESC _ G ... (body collected).
    Graphics,
    /// Saw ESC inside the body: `\` ends it.
    GraphicsEsc,
}

/// A transmission still waiting for `m=0`.
struct Chunks {
    id: u32,
    format: Format,
    columns: u32,
    rows: u32,
    quiet: u8,
    base64: Vec<u8>,
}

/// Reads the PTY, keeping kitty graphics commands for itself.
pub struct Filter {
    file: File,
    state: State,
    body: Vec<u8>,
    /// Body grew past `MAX_ENCODED`: the rest of it is skipped.
    overflow: bool,
    chunks: Option<Chunks>,
    /// Filtered bytes not yet handed to the reader.
    out: Vec<u8>,
    served: usize,
    scratch: Vec<u8>,
    listener: Arc<dyn EventListener + Send + Sync>,
    jobs: mpsc::Sender<Job>,
}

impl Filter {
    fn feed(&mut self, input: &[u8]) {
        let mut rest = input;
        while !rest.is_empty() {
            match self.state {
                State::Ground => match rest.iter().position(|&byte| byte == ESC) {
                    Some(at) => {
                        self.out.extend_from_slice(&rest[..at]);
                        self.state = State::Esc;
                        rest = &rest[at + 1..];
                    }
                    None => {
                        self.out.extend_from_slice(rest);
                        rest = &[];
                    }
                },
                State::Esc => {
                    if rest[0] == b'_' {
                        self.state = State::Apc;
                        rest = &rest[1..];
                    } else {
                        self.out.push(ESC);
                        self.state = State::Ground;
                    }
                }
                State::Apc => {
                    if rest[0] == b'G' {
                        self.state = State::Graphics;
                        self.body.clear();
                        self.overflow = false;
                        rest = &rest[1..];
                    } else {
                        // Not ours: the terminal sees (and ignores) it as before.
                        self.out.extend_from_slice(&[ESC, b'_']);
                        self.state = State::Ground;
                    }
                }
                State::Graphics => match rest.iter().position(|&byte| byte == ESC) {
                    Some(at) => {
                        self.collect(&rest[..at]);
                        self.state = State::GraphicsEsc;
                        rest = &rest[at + 1..];
                    }
                    None => {
                        self.collect(rest);
                        rest = &[];
                    }
                },
                State::GraphicsEsc => {
                    if rest[0] == b'\\' {
                        rest = &rest[1..];
                        let body = std::mem::take(&mut self.body);
                        if !self.overflow {
                            self.command(&body);
                        }
                        self.body = body;
                        self.state = State::Ground;
                    } else {
                        // ESC without `\` cancels the string; that ESC starts the next sequence.
                        log::warn!("graphics: unterminated command dropped");
                        self.state = State::Esc;
                    }
                }
            }
        }
    }

    fn collect(&mut self, bytes: &[u8]) {
        if self.body.len() + bytes.len() > MAX_ENCODED {
            if !self.overflow {
                log::warn!("graphics: command over {MAX_ENCODED} bytes dropped");
            }
            self.overflow = true;
            self.body.clear();
            return;
        }
        self.body.extend_from_slice(bytes);
    }

    /// One `ESC _ G <body> ESC \`.
    fn command(&mut self, body: &[u8]) {
        let (control, payload) = match body.iter().position(|&byte| byte == b';') {
            Some(at) => (&body[..at], &body[at + 1..]),
            None => (body, &[][..]),
        };
        let mut keys: HashMap<u8, &[u8]> = HashMap::new();
        for pair in control.split(|&byte| byte == b',') {
            if let [key, b'=', value @ ..] = pair {
                keys.insert(*key, value);
            }
        }
        let char_of = |key: u8, default: u8| {
            keys.get(&key)
                .and_then(|v| v.first().copied())
                .unwrap_or(default)
        };
        let number = |key: u8| {
            keys.get(&key)
                .and_then(|value| std::str::from_utf8(value).ok()?.parse::<u32>().ok())
                .unwrap_or(0)
        };
        let more = number(b'm') == 1;

        // A continuation chunk carries only `m` (and `q`).
        if let Some(chunks) = &mut self.chunks {
            if chunks.base64.len() + payload.len() > MAX_ENCODED {
                log::warn!(
                    "graphics: picture {} over {MAX_ENCODED} bytes dropped",
                    chunks.id
                );
                self.chunks = None;
                return;
            }
            chunks.base64.extend_from_slice(payload);
            if !more {
                self.finish();
            }
            return;
        }

        let id = number(b'i');
        let quiet = number(b'q').min(2) as u8;
        let medium = char_of(b't', b'd');
        let format = Format {
            file: medium == b'f',
            pixels: if keys.contains_key(&b'f') {
                number(b'f')
            } else {
                32
            },
            width: number(b's'),
            height: number(b'v'),
        };
        match char_of(b'a', b't') {
            // Support query: bytes and files (Claude Code's file query names a real file);
            // shared memory and temp files no.
            b'q' => match medium {
                b'd' => self.reply(id, quiet, "OK"),
                b'f' => match open_file(payload) {
                    Ok(_) => self.reply(id, quiet, "OK"),
                    Err(error) => self.reply(id, quiet, &format!("EBADF:{error}")),
                },
                _ => self.reply(id, quiet, "ENOTSUPPORTED:only direct or file transmission"),
            },
            b't' | b'T' => {
                if !matches!(medium, b'd' | b'f') || ![100, 32, 24].contains(&format.pixels) {
                    self.reply(
                        id,
                        quiet,
                        "ENOTSUPPORTED:only PNG or RGB(A), direct or file",
                    );
                    return;
                }
                if id == 0 {
                    self.reply(id, quiet, "EINVAL:no image id");
                    return;
                }
                self.chunks = Some(Chunks {
                    id,
                    format,
                    columns: number(b'c'),
                    rows: number(b'r'),
                    quiet,
                    base64: payload.to_vec(),
                });
                if !more {
                    self.finish();
                }
            }
            b'd' => match char_of(b'd', b'a') {
                b'i' | b'I' => self.jobs.send(Job::Delete(Some(id))).unwrap_or_default(),
                b'a' | b'A' => self.jobs.send(Job::Delete(None)).unwrap_or_default(),
                _ => {}
            },
            // Placements other than Unicode placeholders aren't drawn.
            _ => {}
        }
    }

    fn finish(&mut self) {
        let Some(chunks) = self.chunks.take() else {
            return;
        };
        self.reply(chunks.id, chunks.quiet, "OK");
        self.jobs
            .send(Job::Decode {
                id: chunks.id,
                base64: chunks.base64,
                format: chunks.format,
                columns: chunks.columns,
                rows: chunks.rows,
            })
            .unwrap_or_default();
    }

    /// kitty's response; `q=1` silences OK, `q=2` everything.
    fn reply(&self, id: u32, quiet: u8, message: &str) {
        let ok = message == "OK";
        if quiet >= 2 || (quiet == 1 && ok) || id == 0 {
            return;
        }
        self.listener
            .send_event(Event::PtyWrite(format!("\x1b_Gi={id};{message}\x1b\\")));
    }
}

impl Read for Filter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // A read that was all graphics reads again, so "nothing" still means would-block or EOF.
        while self.served == self.out.len() {
            self.out.clear();
            self.served = 0;
            self.scratch.resize(buf.len().max(1), 0);
            let got = self.file.read(&mut self.scratch)?;
            if got == 0 {
                return Ok(0);
            }
            let scratch = std::mem::take(&mut self.scratch);
            self.feed(&scratch[..got]);
            self.scratch = scratch;
        }
        let count = buf.len().min(self.out.len() - self.served);
        buf[..count].copy_from_slice(&self.out[self.served..self.served + count]);
        self.served += count;
        Ok(count)
    }
}

/// alacritty's `Pty` with reads going through `Filter`.
pub struct GraphicsPty {
    pty: Pty,
    filter: Filter,
}

impl GraphicsPty {
    pub fn new(
        pty: Pty,
        images: Images,
        listener: impl EventListener + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let listener: Arc<dyn EventListener + Send + Sync> = Arc::new(listener);
        let filter = Filter {
            file: pty.file().try_clone()?,
            state: State::Ground,
            body: Vec::new(),
            overflow: false,
            chunks: None,
            out: Vec::new(),
            served: 0,
            scratch: Vec::new(),
            jobs: decoder(images, listener.clone()),
            listener,
        };
        Ok(Self { pty, filter })
    }
}

impl EventedReadWrite for GraphicsPty {
    type Reader = Filter;
    type Writer = File;

    unsafe fn register(
        &mut self,
        poller: &Arc<polling::Poller>,
        event: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        unsafe { self.pty.register(poller, event, mode) }
    }

    fn reregister(
        &mut self,
        poller: &Arc<polling::Poller>,
        event: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.pty.reregister(poller, event, mode)
    }

    fn deregister(&mut self, poller: &Arc<polling::Poller>) -> io::Result<()> {
        self.pty.deregister(poller)
    }

    fn reader(&mut self) -> &mut Filter {
        &mut self.filter
    }

    fn writer(&mut self) -> &mut File {
        self.pty.writer()
    }
}

impl EventedPty for GraphicsPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl OnResize for GraphicsPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::vte::ansi::Rgb;
    use image::{ImageEncoder, codecs::png::PngEncoder};

    use super::*;

    #[derive(Clone, Default)]
    struct Replies(Arc<Mutex<Vec<String>>>);

    impl EventListener for Replies {
        fn send_event(&self, event: Event) {
            if let Event::PtyWrite(text) = event {
                self.0.lock().unwrap().push(text);
            }
        }
    }

    fn filter() -> (Filter, Images, Replies) {
        let images = Images::default();
        let replies = Replies::default();
        let listener: Arc<dyn EventListener + Send + Sync> = Arc::new(replies.clone());
        let filter = Filter {
            file: File::open("/dev/null").unwrap(),
            state: State::Ground,
            body: Vec::new(),
            overflow: false,
            chunks: None,
            out: Vec::new(),
            served: 0,
            scratch: Vec::new(),
            jobs: decoder(images.clone(), listener.clone()),
            listener,
        };
        (filter, images, replies)
    }

    /// Feeds `input` split at `at`, returns what passes through.
    fn run(filter: &mut Filter, input: &[u8], at: usize) -> Vec<u8> {
        filter.feed(&input[..at]);
        filter.feed(&input[at..]);
        std::mem::take(&mut filter.out)
    }

    fn png_base64(width: u32, height: u32) -> String {
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                &vec![255; (width * height * 4) as usize],
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(png)
    }

    fn wait_for(images: &Images, id: u32) -> Option<Arc<Picture>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(picture) = images.get(id) {
                return Some(picture);
            }
            thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn other_bytes_pass_through_at_any_split() {
        let input = b"a\x1b[31mred\x1b]0;title\x07\x1b_Xother apc\x1b\\\x1b\x1b[0m\xe2\x94\x80z";
        for at in 0..=input.len() {
            let (mut filter, ..) = filter();
            assert_eq!(run(&mut filter, input, at), input.to_vec(), "split at {at}");
        }
    }

    #[test]
    fn queries_answer_bytes_and_real_files() {
        let b64 = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
        let input = format!(
            "a\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\b\x1b_Gi=32,s=1,v=1,a=q,t=f,f=32;{}\x1b\\\
             c\x1b_Gi=33,a=q,t=f;{}\x1b\\\x1b_Gi=34,a=q,t=s;eA==\x1b\\",
            b64("/etc/hosts"),
            b64("/no/such/file"),
        );
        for at in 0..=input.len() {
            let (mut filter, _, replies) = filter();
            assert_eq!(
                run(&mut filter, input.as_bytes(), at),
                b"abc".to_vec(),
                "split at {at}"
            );
            let replies = replies.0.lock().unwrap();
            assert_eq!(replies[0], "\x1b_Gi=31;OK\x1b\\");
            assert_eq!(replies[1], "\x1b_Gi=32;OK\x1b\\");
            assert!(replies[2].starts_with("\x1b_Gi=33;EBADF"));
            assert!(replies[3].starts_with("\x1b_Gi=34;ENOTSUPPORTED"));
        }
    }

    #[test]
    fn png_file_and_raw_rgba_are_stored() {
        let png = base64::engine::general_purpose::STANDARD
            .decode(png_base64(5, 3))
            .unwrap();
        let path = std::env::temp_dir().join(format!("ccp-graphics-{}.png", std::process::id()));
        fs::write(&path, png).unwrap();
        let path64 = base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_bytes());
        let rgba = base64::engine::general_purpose::STANDARD.encode([9u8; 2 * 2 * 4]);
        let (mut filter, images, _) = filter();
        filter.feed(
            format!(
                "\x1b_Ga=T,U=1,q=1,f=100,t=f,i=3,c=5,r=3;{path64}\x1b\\\
                 \x1b_Ga=T,U=1,q=2,f=32,s=2,v=2,i=4,c=1,r=1;{rgba}\x1b\\"
            )
            .as_bytes(),
        );
        let file = wait_for(&images, 3).expect("file picture");
        assert_eq!(file.image.size(0).width.0, 5);
        let raw = wait_for(&images, 4).expect("raw picture");
        assert_eq!(raw.image.size(0).height.0, 2);
        fs::remove_file(path).ok();
    }

    #[test]
    fn chunked_png_is_stored_then_deleted() {
        let base64 = png_base64(6, 4);
        let (first, second) = base64.split_at(base64.len() / 2);
        let input = format!(
            "x\x1b_Ga=T,U=1,q=2,f=100,i=7,c=3,r=2,m=1;{first}\x1b\\\x1b_Gm=0,q=2;{second}\x1b\\y"
        );
        for at in [0, 5, input.len() / 2, input.len() - 3] {
            let (mut filter, images, replies) = filter();
            assert_eq!(run(&mut filter, input.as_bytes(), at), b"xy".to_vec());
            let picture = wait_for(&images, 7).expect("decoded");
            assert_eq!((picture.columns, picture.rows), (3, 2));
            assert_eq!(picture.image.size(0).width.0, 6);
            assert!(replies.0.lock().unwrap().is_empty(), "q=2 is silent");
            filter.feed(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
            let deadline = Instant::now() + Duration::from_secs(5);
            while images.get(7).is_some() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(images.get(7).is_none());
            assert_eq!(images.take_retired(false).len(), 1);
        }
    }

    #[test]
    fn garbage_inside_graphics_never_panics() {
        let mut seed = 0x2545_f491_u32;
        let mut random = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let (mut filter, images, _) = filter();
        for _ in 0..200 {
            let mut input = b"\x1b_G".to_vec();
            if random() % 2 == 0 {
                input.extend_from_slice(b"a=T,f=100,i=9,m=0;");
            }
            input.extend((0..random() % 300).map(|_| random() as u8));
            input.extend_from_slice(b"\x1b\\");
            filter.feed(&input);
        }
        filter.feed(b"\x1b_Ga=T,f=100,i=9;!!!!\x1b\\ok");
        assert!(std::mem::take(&mut filter.out).ends_with(b"ok"));
        thread::sleep(Duration::from_millis(50));
        assert!(images.get(9).is_none());
    }

    #[test]
    fn placeholder_reads_id_row_and_column() {
        let mark = |index: usize| char::from_u32(DIACRITICS[index]).unwrap();
        assert_eq!(
            placeholder(Color::Indexed(12), &[mark(0), mark(5)]),
            Some((12, 0, 5))
        );
        let rgb = Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(
            placeholder(Color::Spec(rgb), &[mark(296), mark(1)]),
            Some((0x010203, 296, 1))
        );
        assert_eq!(placeholder(Color::Indexed(1), &[mark(0)]), None);
        assert_eq!(placeholder(Color::Indexed(1), &['a', mark(0)]), None);
    }
}
