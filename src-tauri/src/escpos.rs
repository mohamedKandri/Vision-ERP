//! ESC/POS encoder. Input is the same printer-agnostic document the receipt preview
//! renders (`src/lib/receipt.ts`), so what you see on screen is what prints.

use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Line {
    Logo { src: String },
    Text {
        text: String,
        #[serde(default)]
        align: Option<Align>,
        #[serde(default)]
        bold: Option<bool>,
        #[serde(default)]
        big: Option<bool>,
    },
    Row {
        left: String,
        right: String,
        #[serde(default)]
        bold: Option<bool>,
        #[serde(default)]
        big: Option<bool>,
    },
    Rule,
    Qr { data: String },
    Feed,
    /// A full-width image (e.g. an Arabic receipt rendered by the till). PNG data URL.
    Raster { src: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codepage {
    Cp437,
    Cp850,
    Cp858,
    Cp1252,
    Ascii,
}

impl Codepage {
    pub fn parse(s: &str) -> Codepage {
        match s {
            "cp437" => Codepage::Cp437,
            "cp858" => Codepage::Cp858,
            "cp1252" => Codepage::Cp1252,
            "ascii" => Codepage::Ascii,
            _ => Codepage::Cp850,
        }
    }
    /// `ESC t n` table number (Epson numbering, used by most clones).
    fn esc_t(self) -> Option<u8> {
        match self {
            Codepage::Cp437 => Some(0),
            Codepage::Cp850 => Some(2),
            Codepage::Cp858 => Some(19),
            Codepage::Cp1252 => Some(16),
            Codepage::Ascii => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drawer {
    /// ESC p 0 25 250 — standard pulse on pin 2.
    Generic,
    /// DLE DC4 0 0 0 — documented by SUNMI for its built-in drawer port.
    Sunmi,
}

#[derive(Debug, Clone, Copy)]
pub struct Cfg {
    pub paper_mm: u32,
    pub codepage: Codepage,
    pub cut: bool,
    pub drawer: Option<Drawer>,
}

impl Cfg {
    pub fn chars(&self) -> usize {
        if self.paper_mm == 58 { 32 } else { 48 }
    }
    pub fn dots(&self) -> u32 {
        if self.paper_mm == 58 { 384 } else { 576 }
    }
}

const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";
const CP850_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø×ƒáíóúñÑªº¿®¬½¼¡«»░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐└┴┬├─┼ãÃ╚╔╩╦╠═╬¤ðÐÊËÈıÍÎÏ┘┌█▄¦Ì▀ÓßÔÒõÕµþÞÚÛÙýÝ¯´\u{ad}±‗¾¶§÷¸°¨·¹³²■\u{a0}";
const CP1252_80_9F: &str = "€\u{81}‚ƒ„…†‡ˆ‰Š‹Œ\u{8d}Ž\u{8f}\u{90}‘’“”•–—˜™š›œ\u{9d}žŸ";

fn table_byte(c: char, cp: Codepage) -> Option<u8> {
    if (c as u32) < 0x80 {
        return Some(c as u8);
    }
    match cp {
        Codepage::Ascii => None,
        Codepage::Cp437 => CP437_HIGH.chars().position(|x| x == c).map(|i| 0x80 + i as u8),
        Codepage::Cp850 => CP850_HIGH.chars().position(|x| x == c).map(|i| 0x80 + i as u8),
        Codepage::Cp858 => {
            if c == '€' {
                Some(0xD5)
            } else if c == 'ı' {
                None
            } else {
                CP850_HIGH.chars().position(|x| x == c).map(|i| 0x80 + i as u8)
            }
        }
        Codepage::Cp1252 => {
            if let Some(i) = CP1252_80_9F.chars().position(|x| x == c) {
                return Some(0x80 + i as u8);
            }
            let u = c as u32;
            if (0xA0..=0xFF).contains(&u) { Some(u as u8) } else { None }
        }
    }
}

/// Characters that never need the code page.
fn normalize(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{202f}' | '\u{a0}' | '\u{2009}' => " ",
        '−' | '–' | '—' => "-",
        '’' | '‘' | '´' => "'",
        '“' | '”' => "\"",
        '…' => "...",
        '•' => "*",
        _ => return None,
    })
}

fn strip_accent(c: char) -> Option<char> {
    const PAIRS: &[(&str, char)] = &[
        ("àâäáãå", 'a'), ("ÀÂÄÁÃÅ", 'A'), ("éèêë", 'e'), ("ÉÈÊË", 'E'), ("îïíì", 'i'), ("ÎÏÍÌ", 'I'),
        ("ôöóòõ", 'o'), ("ÔÖÓÒÕ", 'O'), ("ùûüú", 'u'), ("ÙÛÜÚ", 'U'), ("ç", 'c'), ("Ç", 'C'), ("ñ", 'n'), ("Ñ", 'N'),
        ("ÿý", 'y'), ("×", 'x'), ("œ", 'o'), ("Œ", 'O'),
    ];
    PAIRS.iter().find(|(s, _)| s.contains(c)).map(|(_, r)| *r)
}

/// Encode text for the printer's code page. Unknown letters (e.g. Arabic in text mode)
/// become '?'; Arabic receipts are sent as images instead.
pub fn encode_text(s: &str, cp: Codepage) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        if let Some(n) = normalize(c) {
            out.extend_from_slice(n.as_bytes());
        } else if c == '×' && table_byte(c, cp).is_none() {
            out.push(b'x');
        } else if let Some(b) = table_byte(c, cp) {
            out.push(b);
        } else if let Some(a) = strip_accent(c) {
            out.push(a as u8);
        } else if c == '€' {
            out.extend_from_slice(b"EUR");
        } else {
            out.push(b'?');
        }
    }
    out
}

fn len(s: &str) -> usize {
    s.chars().count()
}

/// Same algorithm as `wrap` in receipt.ts.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.is_empty() {
                line = word.to_string();
            } else if len(&line) + 1 + len(word) <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
            while len(&line) > width {
                let head: String = line.chars().take(width).collect();
                let tail: String = line.chars().skip(width).collect();
                out.push(head);
                line = tail;
            }
        }
        out.push(line);
    }
    out
}

/// Same algorithm as `row` in receipt.ts.
pub fn row(left: &str, right: &str, width: usize) -> Vec<String> {
    let room = width.saturating_sub(len(right) + 1);
    let mut parts = wrap(left, room.max(8));
    let last = parts.pop().unwrap_or_default();
    if len(&last) + 1 + len(right) <= width {
        parts.push(format!("{last}{}{right}", " ".repeat(width - len(&last) - len(right))));
    } else {
        parts.push(last);
        parts.push(format!("{}{right}", " ".repeat(width.saturating_sub(len(right)))));
    }
    parts
}

// ------------------------------------------------------------------ images

pub struct Mono {
    pub width: u32,
    pub height: u32,
    /// 1 = black. Row-major, one byte per pixel (packed when encoding).
    pub px: Vec<u8>,
}

/// Decode a PNG data URL (or raw base64) into grayscale 0..255 on white.
pub fn decode_png(src: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let b64 = src.split_once("base64,").map(|x| x.1).unwrap_or(src);
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| format!("image: {e}"))?;
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("png: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("png: {e}"))?;
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 || w > 4000 || h > 20000 {
        return Err("image size out of range".into());
    }
    let ch = info.color_type.samples();
    let mut gray = Vec::with_capacity((w * h) as usize);
    for i in 0..(w * h) as usize {
        let p = &buf[i * ch..i * ch + ch];
        let (l, a) = match ch {
            1 => (p[0] as u32, 255),
            2 => (p[0] as u32, p[1] as u32),
            3 => ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000, 255),
            _ => ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000, p[3] as u32),
        };
        // Composite on white.
        gray.push(((l * a + 255 * (255 - a)) / 255) as u8);
    }
    Ok((w, h, gray))
}

/// Box-filter downscale to `max_w` (never upscale), then threshold or dither to 1 bit.
pub fn to_mono(w: u32, h: u32, gray: &[u8], max_w: u32, dither: bool) -> Mono {
    let tw = w.min(max_w).max(1);
    let th = ((h as u64 * tw as u64) / w as u64).max(1) as u32;
    let mut scaled = vec![0f32; (tw * th) as usize];
    for y in 0..th {
        let sy0 = (y as u64 * h as u64 / th as u64) as u32;
        let sy1 = (((y + 1) as u64 * h as u64 / th as u64) as u32).max(sy0 + 1).min(h);
        for x in 0..tw {
            let sx0 = (x as u64 * w as u64 / tw as u64) as u32;
            let sx1 = (((x + 1) as u64 * w as u64 / tw as u64) as u32).max(sx0 + 1).min(w);
            let mut sum = 0u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    sum += gray[(sy * w + sx) as usize] as u32;
                }
            }
            scaled[(y * tw + x) as usize] = sum as f32 / ((sy1 - sy0) * (sx1 - sx0)) as f32;
        }
    }
    let mut px = vec![0u8; scaled.len()];
    for y in 0..th as usize {
        for x in 0..tw as usize {
            let i = y * tw as usize + x;
            let old = scaled[i];
            let black = old < if dither { 128.0 } else { 160.0 };
            px[i] = black as u8;
            if dither {
                let err = old - if black { 0.0 } else { 255.0 };
                let mut spread = |dx: i64, dy: i64, f: f32| {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx >= 0 && nx < tw as i64 && ny < th as i64 {
                        scaled[(ny as usize) * tw as usize + nx as usize] += err * f;
                    }
                };
                spread(1, 0, 7.0 / 16.0);
                spread(-1, 1, 3.0 / 16.0);
                spread(0, 1, 5.0 / 16.0);
                spread(1, 1, 1.0 / 16.0);
            }
        }
    }
    Mono { width: tw, height: th, px }
}

/// GS v 0 raster, in bands of 256 rows so small printer buffers cope.
pub fn raster_bytes(m: &Mono) -> Vec<u8> {
    let xb = m.width.div_ceil(8);
    let mut out = Vec::new();
    let mut y0 = 0;
    while y0 < m.height {
        let rows = (m.height - y0).min(256);
        out.extend_from_slice(&[0x1D, 0x76, 0x30, 0x00, (xb & 0xFF) as u8, (xb >> 8) as u8, (rows & 0xFF) as u8, (rows >> 8) as u8]);
        for y in y0..y0 + rows {
            for bx in 0..xb {
                let mut byte = 0u8;
                for bit in 0..8 {
                    let x = bx * 8 + bit;
                    if x < m.width && m.px[(y * m.width + x) as usize] == 1 {
                        byte |= 0x80 >> bit;
                    }
                }
                out.push(byte);
            }
        }
        y0 += rows;
    }
    out
}

// ------------------------------------------------------------------ encoder

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;

fn align_byte(a: Align) -> u8 {
    match a {
        Align::Left => 0,
        Align::Center => 1,
        Align::Right => 2,
    }
}

pub fn qr_bytes(data: &str) -> Vec<u8> {
    let d = data.as_bytes();
    let n = d.len() + 3;
    let mut out = vec![
        ESC, b'a', 1, // center
        GS, 0x28, 0x6B, 4, 0, 0x31, 0x41, 0x32, 0x00, // model 2
        GS, 0x28, 0x6B, 3, 0, 0x31, 0x43, 6, // module size 6
        GS, 0x28, 0x6B, 3, 0, 0x31, 0x45, 0x30, // error correction L
        GS, 0x28, 0x6B, (n & 0xFF) as u8, (n >> 8) as u8, 0x31, 0x50, 0x30,
    ];
    out.extend_from_slice(d);
    out.extend_from_slice(&[GS, 0x28, 0x6B, 3, 0, 0x31, 0x51, 0x30, b'\n', ESC, b'a', 0]);
    out
}

pub fn drawer_bytes(d: Drawer) -> Vec<u8> {
    match d {
        Drawer::Generic => vec![ESC, b'p', 0, 25, 250],
        Drawer::Sunmi => vec![0x10, 0x14, 0, 0, 0],
    }
}

pub fn init_bytes(cfg: &Cfg) -> Vec<u8> {
    let mut out = vec![ESC, b'@'];
    if let Some(n) = cfg.codepage.esc_t() {
        out.extend_from_slice(&[ESC, b't', n]);
    }
    out
}

pub fn cut_bytes() -> Vec<u8> {
    // Feed then partial cut (GS V 66 0): widely supported by Epson-compatible printers.
    vec![ESC, b'd', 3, GS, b'V', 66, 0]
}

/// Encode a document. Images that fail to decode are skipped (the text still prints).
pub fn encode(doc: &[Line], cfg: &Cfg) -> Vec<u8> {
    let w = cfg.chars();
    let mut out = init_bytes(cfg);
    if let Some(d) = cfg.drawer {
        out.extend(drawer_bytes(d));
    }
    let style = |out: &mut Vec<u8>, align: Align, bold: bool, big: bool| {
        out.extend_from_slice(&[ESC, b'a', align_byte(align), ESC, b'E', bold as u8, GS, b'!', if big { 0x11 } else { 0x00 }]);
    };
    for l in doc {
        match l {
            Line::Text { text, align, bold, big } => {
                let big = big.unwrap_or(false);
                style(&mut out, align.unwrap_or_default(), bold.unwrap_or(false), big);
                for part in wrap(text, if big { w / 2 } else { w }) {
                    out.extend(encode_text(&part, cfg.codepage));
                    out.push(b'\n');
                }
            }
            Line::Row { left, right, bold, big } => {
                let big = big.unwrap_or(false);
                style(&mut out, Align::Left, bold.unwrap_or(false), big);
                for part in row(left, right, if big { w / 2 } else { w }) {
                    out.extend(encode_text(&part, cfg.codepage));
                    out.push(b'\n');
                }
            }
            Line::Rule => {
                style(&mut out, Align::Left, false, false);
                out.extend(std::iter::repeat_n(b'-', w));
                out.push(b'\n');
            }
            Line::Feed => out.push(b'\n'),
            Line::Qr { data } => out.extend(qr_bytes(data)),
            Line::Logo { src } | Line::Raster { src } => {
                let logo = matches!(l, Line::Logo { .. });
                if let Ok((iw, ih, g)) = decode_png(src) {
                    let max = if logo { cfg.dots() * 6 / 10 } else { cfg.dots() };
                    let m = to_mono(iw, ih, &g, max, logo);
                    out.extend_from_slice(&[ESC, b'a', 1]);
                    out.extend(raster_bytes(&m));
                    out.extend_from_slice(&[ESC, b'a', 0]);
                }
            }
        }
    }
    style(&mut out, Align::Left, false, false);
    if cfg.cut {
        out.extend(cut_bytes());
    } else {
        out.extend_from_slice(&[ESC, b'd', 4]);
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn png_data_url(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> String {
        let mut data = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut data, w, h);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            let px: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect();
            wr.write_image_data(&px).unwrap();
        }
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(data))
    }

    #[test]
    fn code_pages_for_french() {
        assert_eq!(encode_text("é", Codepage::Cp850), vec![0x82]);
        assert_eq!(encode_text("é", Codepage::Cp437), vec![0x82]);
        assert_eq!(encode_text("é", Codepage::Cp1252), vec![0xE9]);
        assert_eq!(encode_text("à ç È", Codepage::Cp850), vec![0x85, b' ', 0x87, b' ', 0xD4]);
        assert_eq!(encode_text("€", Codepage::Cp858), vec![0xD5]);
        assert_eq!(encode_text("€", Codepage::Cp1252), vec![0x80]);
        assert_eq!(encode_text("€", Codepage::Cp850), b"EUR".to_vec());
        assert_eq!(encode_text("×2", Codepage::Cp850), vec![0x9E, b'2']);
        assert_eq!(encode_text("×2", Codepage::Cp437), b"x2".to_vec());
        assert_eq!(encode_text("Crème brûlée", Codepage::Ascii), b"Creme brulee".to_vec());
        // French money formatting uses narrow no-break spaces.
        assert_eq!(encode_text("1\u{202f}234,00 DH", Codepage::Cp850), b"1 234,00 DH".to_vec());
        assert_eq!(encode_text("− Sans oignon", Codepage::Cp850), b"- Sans oignon".to_vec());
        // Arabic can't print in text mode.
        assert_eq!(encode_text("جبن", Codepage::Cp850), b"???".to_vec());
    }

    #[test]
    fn code_page_tables_have_128_entries() {
        assert_eq!(CP437_HIGH.chars().count(), 128);
        assert_eq!(CP850_HIGH.chars().count(), 128);
        assert_eq!(CP1252_80_9F.chars().count(), 32);
    }

    #[test]
    fn wrap_and_row_match_the_preview() {
        assert_eq!(wrap(&"x".repeat(70), 32), vec!["x".repeat(32), "x".repeat(32), "x".repeat(6)]);
        let r = row("TOTAL", "85,50 DH", 32);
        assert_eq!(r, vec![format!("TOTAL{}85,50 DH", " ".repeat(32 - 5 - 8))]);
        for l in row("2 x Menu Signature avec un nom vraiment très long", "1 234,00 DH", 32) {
            assert!(l.chars().count() <= 32, "{l}");
        }
    }

    #[test]
    fn encodes_text_styles_rule_and_cut() {
        let cfg = Cfg { paper_mm: 58, codepage: Codepage::Cp850, cut: true, drawer: None };
        let doc = vec![
            Line::Text { text: "#42".into(), align: Some(Align::Center), bold: Some(true), big: Some(true) },
            Line::Rule,
        ];
        let b = encode(&doc, &cfg);
        assert_eq!(&b[..5], &[ESC, b'@', ESC, b't', 2]);
        let center_bold_big = [ESC, b'a', 1, ESC, b'E', 1, GS, b'!', 0x11];
        assert!(b.windows(9).any(|w| w == center_bold_big));
        assert!(b.windows(4).any(|w| w == b"#42\n"));
        assert!(b.windows(33).any(|w| w[..32].iter().all(|c| *c == b'-') && w[32] == b'\n'));
        assert_eq!(&b[b.len() - 7..], &[ESC, b'd', 3, GS, b'V', 66, 0]);
    }

    #[test]
    fn big_text_wraps_at_half_width() {
        let cfg = Cfg { paper_mm: 58, codepage: Codepage::Cp850, cut: false, drawer: None };
        let b = encode(&[Line::Text { text: "A".repeat(20), align: None, bold: None, big: Some(true) }], &cfg);
        assert!(b.windows(17).any(|w| w == format!("{}\n", "A".repeat(16)).as_bytes()));
        assert!(b.windows(5).any(|w| w == b"\nAAAA"));
    }

    #[test]
    fn drawer_kicks() {
        let cfg = Cfg { paper_mm: 80, codepage: Codepage::Cp850, cut: false, drawer: Some(Drawer::Generic) };
        let b = encode(&[], &cfg);
        assert!(b.windows(5).any(|w| w == [ESC, b'p', 0, 25, 250]));
        let cfg = Cfg { drawer: Some(Drawer::Sunmi), ..cfg };
        assert!(encode(&[], &cfg).windows(5).any(|w| w == [0x10, 0x14, 0, 0, 0]));
    }

    #[test]
    fn qr_uses_native_command() {
        let q = qr_bytes("BURGEROS|42");
        let store = [GS, 0x28, 0x6B, 14, 0, 0x31, 0x50, 0x30];
        assert!(q.windows(8).any(|w| w == store));
        assert!(q.windows(11).any(|w| w == b"BURGEROS|42"));
    }

    #[test]
    fn png_to_raster() {
        // 20x10 image: left half black, right half white.
        let url = png_data_url(20, 10, |x, _| if x < 10 { 0 } else { 255 });
        let (w, h, g) = decode_png(&url).unwrap();
        assert_eq!((w, h), (20, 10));
        let m = to_mono(w, h, &g, 576, false);
        let r = raster_bytes(&m);
        assert_eq!(&r[..8], &[GS, 0x76, 0x30, 0, 3, 0, 10, 0]); // 3 bytes per row, 10 rows
        assert_eq!(&r[8..11], &[0xFF, 0xC0, 0x00]); // 10 black pixels then white
        assert_eq!(r.len(), 8 + 3 * 10);
    }

    #[test]
    fn rgba_png_from_a_canvas_decodes() {
        // Browser canvases export RGBA; transparent pixels must print as white.
        let mut data = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut data, 2, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[0, 0, 0, 255, 0, 0, 0, 0]).unwrap();
        }
        let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(data));
        let (w, h, g) = decode_png(&url).unwrap();
        assert_eq!((w, h, g), (2, 1, vec![0, 255]));
    }

    #[test]
    fn wide_images_are_scaled_to_paper_and_banded() {
        let url = png_data_url(1152, 600, |_, y| if y % 2 == 0 { 0 } else { 255 });
        let (w, h, g) = decode_png(&url).unwrap();
        let m = to_mono(w, h, &g, 576, false);
        assert_eq!((m.width, m.height), (576, 300));
        let r = raster_bytes(&m);
        // Two bands: 256 + 44 rows of 72 bytes.
        assert_eq!(r.len(), 8 + 72 * 256 + 8 + 72 * 44);
    }

    #[test]
    fn bad_images_are_skipped_not_fatal() {
        let cfg = Cfg { paper_mm: 80, codepage: Codepage::Cp850, cut: false, drawer: None };
        let b = encode(&[Line::Logo { src: "data:image/png;base64,AAAA".into() }, Line::Text { text: "ok".into(), align: None, bold: None, big: None }], &cfg);
        assert!(b.windows(3).any(|w| w == b"ok\n"));
        assert!(decode_png("not base64 !!").is_err());
    }

    #[test]
    fn doc_json_matches_frontend_receipt_lines() {
        let json = r#"[{"kind":"text","text":"Burger Lab","align":"center","bold":true,"big":true},
            {"kind":"row","left":"TOTAL TTC","right":"85,50 DH","bold":true},{"kind":"rule"},{"kind":"feed"},
            {"kind":"qr","data":"x"},{"kind":"logo","src":""}]"#;
        let doc: Vec<Line> = serde_json::from_str(json).unwrap();
        assert_eq!(doc.len(), 6);
    }
}
