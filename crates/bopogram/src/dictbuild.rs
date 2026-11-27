//! `bopogram build-dict`: the pronunciation dictionary, built on the user's
//! machine from the Ministry of Education's dictionaries as downloaded (their
//! licence lets them be passed on only unchanged, so the built `dict.tsv` is
//! never distributed) and `dict_extra.tsv`, which holds everything taken
//! from other sources (`prototype/docs/release-spec.md`).
//!
//! A port of the `build` step of `training/dict/build_dict.py`; the output
//! must be byte-identical. Python's dictionaries and counters keep insertion
//! order and several steps depend on it (which reading is tried first, ties
//! in a ranking), so the same orders are kept here (`Ordered`).
//!
//! Outputs in OUT_DIR: `dict.tsv` (word, syllables, source tag),
//! `rejected.tsv`, `need_reading.tsv`, `stats.json`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

use calamine::{Data, Reader, Xlsx};

const MAX_WORD_LEN: usize = 8;
const ERHUA_READING: &str = "ㄦˊ";
/// A reading used this often in a position is taken as certain...
const DOMINANCE_SHARE: f64 = 0.9;
/// ...once it has been seen this many times.
const DOMINANCE_MIN_COUNT: u64 = 3;
const SYMBOLS: &str = "ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ";

/// A map that remembers the order keys were first inserted (Python's dict).
#[derive(Clone)]
struct Ordered<K, V> {
    keys: Vec<K>,
    values: Vec<V>,
    index: HashMap<K, usize>,
}

impl<K: Clone + Eq + Hash, V: Clone> Ordered<K, V> {
    fn new() -> Self {
        Ordered { keys: Vec::new(), values: Vec::new(), index: HashMap::new() }
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.index.get(key).map(|&i| &self.values[i])
    }

    fn contains(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    /// A new key goes last; an existing one keeps its place.
    fn insert(&mut self, key: K, value: V) {
        match self.index.get(&key) {
            Some(&i) => self.values[i] = value,
            None => {
                self.index.insert(key.clone(), self.keys.len());
                self.keys.push(key);
                self.values.push(value);
            }
        }
    }

    fn get_mut_or(&mut self, key: K, default: V) -> &mut V {
        if !self.index.contains_key(&key) {
            self.insert(key.clone(), default);
        }
        let i = self.index[&key];
        &mut self.values[i]
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.keys.iter().zip(self.values.iter())
    }

    /// The items as they are now (`list(d.items())`).
    fn snapshot(&self) -> Vec<(K, V)> {
        self.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// Python's `collections.Counter` as used here.
type Counter = Ordered<String, u64>;

impl Counter {
    fn add(&mut self, key: &str, n: u64) {
        *self.get_mut_or(key.to_owned(), 0) += n;
    }

    fn count(&self, key: &str) -> u64 {
        self.get(&key.to_owned()).copied().unwrap_or(0)
    }

    /// Most common first; equal counts in insertion order (`most_common()`).
    fn most_common(&self) -> Vec<(String, u64)> {
        let mut items = self.snapshot();
        items.sort_by(|a, b| b.1.cmp(&a.1));
        items
    }

    fn total(&self) -> u64 {
        self.values.iter().sum()
    }
}

// -- what Python's regular expressions and str methods do here ---------------

/// `str.isspace()`.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// `\d`: a decimal digit.
fn py_digit(c: char) -> bool {
    c.is_ascii_digit() || ('０'..='９').contains(&c)
}

fn is_bopomofo(c: char) -> bool {
    ('ㄅ'..='ㄩ').contains(&c)
}

/// `^˙?[ㄅ-ㄩ]{1,3}[ˊˇˋ]?$`
fn is_syllable(s: &str) -> bool {
    let body = s.strip_prefix('˙').unwrap_or(s);
    let body = body.strip_suffix(['ˊ', 'ˇ', 'ˋ']).unwrap_or(body);
    let n = body.chars().count();
    (1..=3).contains(&n) && body.chars().all(is_bopomofo)
}

/// `^[一-鿿㐀-䶿]+$`
fn is_cjk_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| ('一'..='\u{9fff}').contains(&c) || ('㐀'..='䶿').contains(&c))
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// MOE bopomofo field -> syllables. Detaches a suffixed erhua 'ㄦ'.
fn split_syllables(field: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in py_strip(field).split(|c: char| py_space(c) || c == '\u{3000}') {
        if tok.is_empty() {
            continue;
        }
        match tok.strip_suffix('ㄦ') {
            Some(head) if is_syllable(head) => {
                out.push(head.to_owned());
                out.push(ERHUA_READING.to_owned());
            }
            _ => out.push(tok.to_owned()),
        }
    }
    out
}

fn is_open(c: char) -> bool {
    c == '（' || c == '('
}

fn is_close(c: char) -> bool {
    c == '）' || c == ')'
}

/// A field may hold several readings separated by markers such as （變）:
/// `re.split(r"[（(][^）)]*[）)]", field)`.
fn readings_from(field: Option<&str>) -> Vec<Vec<String>> {
    let Some(field) = field.filter(|f| !f.is_empty()) else { return Vec::new() };
    let chars: Vec<char> = field.chars().collect();
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut i = 0;
    while i < chars.len() {
        if is_open(chars[i]) {
            if let Some(j) = (i + 1..chars.len()).find(|&j| is_close(chars[j])) {
                parts.push(std::mem::take(&mut part));
                i = j + 1;
                continue;
            }
        }
        part.push(chars[i]);
        i += 1;
    }
    parts.push(part);
    parts.iter().map(|p| split_syllables(p)).filter(|s| !s.is_empty()).collect()
}

/// Drop sense markers like 行(1): `re.sub(r"[（(].*?[）)]|\d+$", "", word)`.
fn clean_word(raw: Option<&str>) -> String {
    let chars: Vec<char> = py_strip(raw.unwrap_or("")).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if is_open(chars[i]) {
            let close = (i + 1..chars.len()).take_while(|&j| chars[j] != '\n').find(|&j| is_close(chars[j]));
            if let Some(j) = close {
                i = j + 1;
                continue;
            }
        }
        if py_digit(chars[i]) && chars[i..].iter().all(|&c| py_digit(c)) {
            break;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// -- reading the MOE tables ---------------------------------------------------

/// Where a table is: an unpacked .xlsx, or a member of a downloaded .zip.
#[derive(Clone, Debug)]
enum Source {
    File(PathBuf),
    Zipped(PathBuf, String),
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            files_under(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// `fnmatch` with one `*`.
fn matches(name: &str, pattern: &str) -> bool {
    let (prefix, suffix) = pattern.split_once('*').unwrap_or((pattern, ""));
    name.len() >= prefix.len() + suffix.len() && name.starts_with(prefix) && name.ends_with(suffix)
}

fn bad(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// The newest MOE table matching `pattern`, unpacked or inside its .zip.
fn find_moe(moe_dir: &Path, pattern: &str) -> io::Result<Source> {
    let mut files = Vec::new();
    files_under(moe_dir, &mut files)?;
    let mut hits: Vec<(String, Source)> = Vec::new();
    for path in &files {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if matches(&name, pattern) && !name.contains("欄位說明") {
            hits.push((name, Source::File(path.clone())));
        }
    }
    for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == "zip")) {
        let archive = zip::ZipArchive::new(fs::File::open(path)?).map_err(|e| bad(format!("{}: {e}", path.display())))?;
        let names: Vec<String> = archive
            .file_names()
            .map(|n| n.map(|n| n.into_owned()))
            .collect::<Result<_, _>>()
            .map_err(|e| bad(format!("{}: {e}", path.display())))?;
        for member in &names {
            if matches(base_name(member), pattern) && !member.contains("欄位說明") {
                hits.push((base_name(member).to_owned(), Source::Zipped(path.clone(), member.to_owned())));
            }
        }
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0));
    hits.pop().map(|(_, source)| source).ok_or_else(|| bad(format!("missing {pattern} under {}", moe_dir.display())))
}

fn read_bytes(source: &Source) -> io::Result<Vec<u8>> {
    match source {
        Source::File(path) => fs::read(path),
        Source::Zipped(path, member) => {
            let mut archive = zip::ZipArchive::new(fs::File::open(path)?).map_err(|e| bad(format!("{}: {e}", path.display())))?;
            let mut file = archive.by_name(member).map_err(|e| bad(format!("{}: {member}: {e}", path.display())))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        }
    }
}

/// A cell as Python's `str()` of what openpyxl gives; `None` for an empty cell.
fn cell_text(cell: &Data) -> Option<String> {
    match cell {
        Data::Empty => None,
        Data::String(s) => Some(s.clone()),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => Some(format!("{}", *f as i64)),
        Data::Float(f) => Some(format!("{f}")),
        Data::Int(i) => Some(i.to_string()),
        Data::Bool(b) => Some(if *b { "True" } else { "False" }.to_owned()),
        other => Some(other.to_string()),
    }
}

/// The rows of a table's first sheet, as (word, main reading, other readings).
type Row = (String, Vec<Vec<String>>, Vec<Vec<String>>);

fn iter_moe(source: &Source, word_col: &str, bpmf_col: &str, var_col: Option<&str>) -> io::Result<Vec<Row>> {
    let bytes = read_bytes(source)?;
    let mut book: Xlsx<_> = Xlsx::new(Cursor::new(bytes)).map_err(|e| bad(format!("{source:?}: {e}")))?;
    let range = book
        .worksheet_range_at(0)
        .ok_or_else(|| bad(format!("{source:?}: no sheet")))?
        .map_err(|e| bad(format!("{source:?}: {e}")))?;
    let mut rows = range.rows();
    let header: Vec<String> =
        rows.next().unwrap_or_default().iter().map(|c| cell_text(c).map(|t| py_strip(&t).to_owned()).unwrap_or_default()).collect();
    let column = |name: &str| header.iter().position(|h| h == name).ok_or_else(|| bad(format!("{source:?}: no column {name}")));
    let (wi, bi) = (column(word_col)?, column(bpmf_col)?);
    let vi = var_col.map(column).transpose()?;
    let mut out = Vec::new();
    for row in rows {
        let text = |i: usize| row.get(i).and_then(cell_text);
        let Some(word) = text(wi) else { continue };
        let main = readings_from(text(bi).as_deref());
        let variants = vi.map(|v| readings_from(text(v).as_deref())).unwrap_or_default();
        // In the idiom table variants follow the main reading in the same field.
        let rest: Vec<Vec<String>> = main.iter().skip(1).cloned().chain(variants).collect();
        out.push((clean_word(Some(&word)), main.into_iter().take(1).collect(), rest));
    }
    Ok(out)
}

// -- building -------------------------------------------------------------------

type Key = (String, Vec<String>);

fn priority(tag: &str) -> u8 {
    match tag.split(':').next().unwrap_or(tag) {
        "concised" => 0,
        "idiom" => 1,
        "revised" => 2,
        "manual" => 3,
        "web" => 4,
        other => panic!("no priority for source {other:?}"),
    }
}

fn base_tag(tag: &str) -> &str {
    tag.split(':').next().unwrap_or(tag)
}

struct Builder {
    entries: Ordered<Key, String>,
    rejected: Vec<[String; 4]>,
    stats: Counter,
}

impl Builder {
    fn add(&mut self, word: &str, syls: &[String], tag: &str) -> bool {
        if !is_cjk_word(word) || char_len(word) > MAX_WORD_LEN {
            self.stats.add("skipped_non_cjk_or_long", 1);
            return false;
        }
        if syls.len() != char_len(word) || !syls.iter().all(|s| is_syllable(s)) {
            self.rejected.push([word.to_owned(), syls.join(" "), tag.to_owned(), "misaligned".to_owned()]);
            return false;
        }
        let key = (word.to_owned(), syls.to_vec());
        let replace = match self.entries.get(&key) {
            None => true,
            Some(old) => {
                priority(tag) < priority(old)
                    || (old.ends_with(":var") && !tag.ends_with(":var") && priority(tag) <= priority(old))
            }
        };
        if replace {
            self.entries.insert(key, tag.to_owned());
        }
        true
    }
}

/// `dict_extra.tsv`: lines of kind <TAB> fields...
fn read_extra(path: &Path) -> io::Result<HashMap<String, Vec<Vec<String>>>> {
    let text = fs::read_to_string(path)?;
    let mut extra: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t').map(str::to_owned);
        let kind = fields.next().unwrap_or_default();
        extra.entry(kind).or_default().push(fields.collect());
    }
    Ok(extra)
}

fn toneless(s: &str) -> &str {
    s.trim_start_matches('˙').trim_end_matches(['ˊ', 'ˇ', 'ˋ'])
}

fn tone_of(syl: &str) -> u8 {
    if syl.starts_with('˙') {
        return 5;
    }
    match syl.chars().last() {
        Some('ˊ') => 2,
        Some('ˇ') => 3,
        Some('ˋ') => 4,
        _ => 1,
    }
}

/// The tone people say (and type) for 一 and 不 before `next`.
fn spoken<'a>(ch: char, syl: &'a str, next: &str) -> &'a str {
    if ch == '不' && syl == "ㄅㄨˋ" && tone_of(next) == 4 {
        return "ㄅㄨˊ";
    }
    if ch == '一' && syl == "ㄧ" {
        if tone_of(next) == 4 || next == "˙ㄍㄜ" {
            return "ㄧˊ";
        }
        if matches!(tone_of(next), 1..=3) {
            return "ㄧˋ";
        }
    }
    syl
}

/// `itertools.product(*options)`, the first `limit` combinations.
fn product(options: &[Vec<String>], limit: usize) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    if options.iter().any(Vec::is_empty) {
        return out;
    }
    let mut at = vec![0usize; options.len()];
    while out.len() < limit {
        out.push(at.iter().zip(options).map(|(&i, o)| o[i].clone()).collect());
        let mut k = options.len();
        loop {
            if k == 0 {
                return out;
            }
            k -= 1;
            at[k] += 1;
            if at[k] < options[k].len() {
                break;
            }
            at[k] = 0;
        }
    }
    out
}

pub struct Summary {
    pub entries: usize,
    pub rejected: usize,
    pub need_reading: usize,
}

/// Build the dictionary from the MOE tables under `moe_dir` and `extra_path`.
pub fn build(moe_dir: &Path, extra_path: &Path, out_dir: &Path, progress: &mut dyn Write) -> io::Result<Summary> {
    fs::create_dir_all(out_dir)?;
    let extra = read_extra(extra_path)?;
    let kind = |name: &str| extra.get(name).map(Vec::as_slice).unwrap_or_default();
    let sources = [
        ("concised", find_moe(moe_dir, "dict_concised_*.xlsx")?, "字詞名", "注音一式", Some("變體注音")),
        ("idiom", find_moe(moe_dir, "dict_idioms_*.xlsx")?, "成語", "注音", None),
        ("revised", find_moe(moe_dir, "dict_revised_*.xlsx")?, "字詞名", "注音一式", Some("變體注音")),
    ];
    let mut b = Builder { entries: Ordered::new(), rejected: Vec::new(), stats: Counter::new() };

    for (tag, source, wcol, bcol, vcol) in &sources {
        let var_tag = format!("{tag}:var");
        for (word, main, variants) in iter_moe(source, wcol, bcol, *vcol)? {
            b.stats.add(&format!("{tag}_rows"), 1);
            for syls in &main {
                b.add(&word, syls, tag);
            }
            for syls in &variants {
                b.add(&word, syls, &var_tag);
            }
        }
        let _ = writeln!(progress, "{tag}: {} rows, {} entries so far", b.stats.count(&format!("{tag}_rows")), b.entries.len());
    }

    // Per-character readings, used to give readings to added words.
    let mut char_readings: Ordered<String, Ordered<String, String>> = Ordered::new();
    for ((word, syls), tag) in b.entries.iter() {
        if char_len(word) == 1 {
            char_readings.get_mut_or(word.clone(), Ordered::new()).insert(syls[0].clone(), tag.clone());
        }
    }
    // "Common" readings: non-variant readings listed by the concise dictionary.
    let common: HashMap<String, Vec<String>> = char_readings
        .iter()
        .map(|(c, r)| (c.clone(), r.iter().filter(|(_, t)| *t == "concised").map(|(s, _)| s.clone()).collect()))
        .collect();
    // How each character is read inside everyday words, by position (word-final or not).
    let mut usage: HashMap<(char, bool), Counter> = HashMap::new();
    for ((word, syls), tag) in b.entries.iter() {
        let n = char_len(word);
        if n >= 2 && (tag == "concised" || tag == "idiom") {
            for (i, (ch, syl)) in word.chars().zip(syls).enumerate() {
                usage.entry((ch, i == n - 1)).or_insert_with(Counter::new).add(syl, 1);
            }
        }
    }
    let mut overrides: HashMap<String, String> = HashMap::new();
    let readings_of = |ch: char| char_readings.get(&ch.to_string());

    // Neutral-tone syllables: many people type the character's own tone instead.
    let mut alt_added = 0;
    for ((word, syls), tag) in b.entries.snapshot() {
        if char_len(&word) < 2 || tag.ends_with(":var") || !syls.iter().any(|s| s.starts_with('˙')) {
            continue;
        }
        let mut options: Vec<Vec<String>> = Vec::new();
        for (ch, syl) in word.chars().zip(&syls) {
            let mut alts = vec![syl.clone()];
            if syl.starts_with('˙') {
                let same_base: Vec<String> = readings_of(ch)
                    .map(|r| {
                        r.iter().map(|(s, _)| s).filter(|s| !s.starts_with('˙') && toneless(s) == toneless(syl)).cloned().collect()
                    })
                    .unwrap_or_default();
                let common_ch = common.get(&ch.to_string());
                let preferred: Vec<String> =
                    same_base.iter().filter(|r| common_ch.is_some_and(|c| c.contains(r))).cloned().collect();
                alts = if !preferred.is_empty() {
                    preferred
                } else if !same_base.is_empty() {
                    same_base
                } else {
                    vec![syl.clone()]
                };
            }
            options.push(alts);
        }
        for new in product(&options, 6) {
            let key = (word.clone(), new);
            if key.1 != syls && !b.entries.contains(&key) {
                b.entries.insert(key, format!("{}:alt", base_tag(&tag)));
                alt_added += 1;
            }
        }
    }
    b.stats.add("neutral_tone_alternatives", alt_added);

    // Candidate readings of `ch` in an added word; one item means confident.
    let guess = |overrides: &HashMap<String, String>, ch: char, last: bool| -> Vec<String> {
        if let Some(o) = overrides.get(&ch.to_string()) {
            return vec![o.clone()];
        }
        let empty = Counter::new();
        let seen = usage.get(&(ch, last)).unwrap_or(&empty);
        let total = seen.total();
        let ranked: Vec<(String, u64)> = seen.most_common();
        if total >= DOMINANCE_MIN_COUNT {
            let (top, n) = &ranked[0];
            if *n as f64 / total as f64 >= DOMINANCE_SHARE {
                return vec![top.clone()];
            }
        }
        let opts: Vec<String> = match common.get(&ch.to_string()).filter(|c| !c.is_empty()) {
            Some(c) => c.clone(),
            None => readings_of(ch).map(|r| r.keys.clone()).unwrap_or_default(),
        };
        if opts.len() == 1 {
            return opts;
        }
        let mut out: Vec<String> = ranked.into_iter().map(|(s, _)| s).collect();
        let extra: Vec<String> = opts.into_iter().filter(|s| !out.contains(s)).collect();
        out.extend(extra);
        out
    };

    for fields in kind("manual") {
        let (word, bpmf) = (&fields[0], &fields[1]);
        let syls: Vec<String> = bpmf.split(py_space).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        b.add(word, &syls, "manual");
        b.stats.add("manual", 1);
        if char_len(word) == 1 {
            overrides.insert(word.clone(), bpmf.clone());
        }
    }

    let mut need: Vec<(String, i64, String)> = Vec::new();
    let known: HashSet<String> = b.entries.keys.iter().map(|(w, _)| w.clone()).collect();
    for fields in kind("twngrams") {
        let (word, count) = (&fields[0], &fields[1]);
        if known.contains(word) {
            continue;
        }
        let n = char_len(word);
        let options: Vec<Vec<String>> = word.chars().enumerate().map(|(i, c)| guess(&overrides, c, i == n - 1)).collect();
        if options.iter().all(|o| o.len() == 1) {
            let syls: Vec<String> = options.iter().map(|o| o[0].clone()).collect();
            b.add(word, &syls, "web");
            b.stats.add("web_auto", 1);
        } else {
            let shown: Vec<String> =
                options.iter().map(|o| if o.is_empty() { "?".to_owned() } else { o.join("/") }).collect();
            let count: i64 = count.trim().parse().map_err(|_| bad(format!("dict_extra.tsv: bad count for {word}")))?;
            need.push((word.clone(), count, shown.join(" ")));
            b.stats.add("web_need_reading", 1);
        }
    }

    // Everyday spellings: the MOE tables write 臺, most people write 台.
    let mut twins = 0;
    for ((word, syls), tag) in b.entries.snapshot() {
        let twin = word.replace('臺', "台");
        let key = (twin, syls);
        if key.0 != word && !b.entries.contains(&key) {
            b.entries.insert(key, tag);
            twins += 1;
        }
    }
    b.stats.add("everyday_spelling_twins", twins);

    // Spellings people type that differ from the dictionary reading.
    let mut variants: HashMap<(String, String), String> = HashMap::new();
    for fields in kind("variant") {
        variants.insert((fields[0].clone(), fields[1].clone()), fields[2].clone());
    }
    let mut typed_added = 0;
    for ((word, syls), tag) in b.entries.snapshot() {
        let pairs: Vec<(String, String)> = word.chars().zip(&syls).map(|(c, s)| (c.to_string(), s.clone())).collect();
        if !pairs.iter().any(|p| variants.contains_key(p)) {
            continue;
        }
        let typed: Vec<String> = pairs.iter().map(|p| variants.get(p).unwrap_or(&p.1).clone()).collect();
        let key = (word.clone(), typed);
        if !b.entries.contains(&key) {
            let new_tag = if tag.ends_with(":var") { tag.clone() } else { format!("{}:alt", base_tag(&tag)) };
            b.entries.insert(key, new_tag);
            typed_added += 1;
        }
    }
    b.stats.add("typing_variants", typed_added);

    // Characters missing from the dictionaries above, from the CNS 11643 database.
    let have: HashSet<String> = b.entries.keys.iter().filter(|(w, _)| char_len(w) == 1).map(|(w, _)| w.clone()).collect();
    let mut cns_chars: HashSet<String> = HashSet::new();
    for fields in kind("cns") {
        let (ch, syl) = (&fields[0], &fields[1]);
        if !have.contains(ch) {
            b.entries.insert((ch.clone(), vec![syl.clone()]), "cns".to_owned());
            cns_chars.insert(ch.clone());
        }
    }
    b.stats.add("cns_chars", cns_chars.len() as u64);

    // Tone sandhi of 一 and 不.
    let mut sandhi_added = 0;
    for ((word, syls), tag) in b.entries.snapshot() {
        let chars: Vec<char> = word.chars().collect();
        let head = &chars[..chars.len().saturating_sub(1)];
        if chars.len() < 2 || tag.ends_with(":var") || !(head.contains(&'一') || head.contains(&'不')) {
            continue;
        }
        let new: Vec<String> = (0..chars.len())
            .map(|i| if i + 1 < chars.len() { spoken(chars[i], &syls[i], &syls[i + 1]).to_owned() } else { syls[i].clone() })
            .collect();
        let key = (word.clone(), new);
        if key.1 != syls && !b.entries.contains(&key) {
            b.entries.insert(key, format!("{}:alt", base_tag(&tag)));
            sandhi_added += 1;
        }
    }
    for (ch, syl) in [("一", "ㄧˊ"), ("一", "ㄧˋ"), ("不", "ㄅㄨˊ")] {
        let key = (ch.to_owned(), vec![syl.to_owned()]);
        if !b.entries.contains(&key) {
            b.entries.insert(key, "concised:var".to_owned());
        }
    }
    b.stats.add("sandhi_alternatives", sandhi_added);

    // Bopomofo symbols themselves.
    for sym in SYMBOLS.chars() {
        b.entries.insert((sym.to_string(), vec![sym.to_string()]), "symbol".to_owned());
    }

    write_outputs(&mut b, &mut need, out_dir)?;
    Ok(Summary { entries: b.entries.len(), rejected: b.rejected.len(), need_reading: need.len() })
}

fn write_outputs(b: &mut Builder, need: &mut [(String, i64, String)], out_dir: &Path) -> io::Result<()> {
    let mut ordered: Vec<(&Key, &String)> = b.entries.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(b.0));
    let mut text = String::with_capacity(8 << 20);
    for ((word, syls), tag) in &ordered {
        text.push_str(word);
        text.push('\t');
        text.push_str(&syls.join(" "));
        text.push('\t');
        text.push_str(tag);
        text.push('\n');
    }
    // Written beside and renamed: a dictionary is never left half written.
    let tmp = out_dir.join("dict.tsv.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, out_dir.join("dict.tsv"))?;

    let rejected: String = b.rejected.iter().map(|row| row.join("\t") + "\n").collect();
    fs::write(out_dir.join("rejected.tsv"), rejected)?;
    need.sort_by(|a, b| b.1.cmp(&a.1));
    let need_text: String = need.iter().map(|(w, c, o)| format!("{w}\t{c}\t{o}\n")).collect();
    fs::write(out_dir.join("need_reading.tsv"), need_text)?;

    let words: HashSet<&str> = b.entries.keys.iter().map(|(w, _)| w.as_str()).collect();
    let syllables: HashSet<&str> = b.entries.keys.iter().flat_map(|(_, s)| s.iter().map(String::as_str)).collect();
    let mut per_word: HashMap<&str, usize> = HashMap::new();
    for (w, _) in &b.entries.keys {
        *per_word.entry(w.as_str()).or_default() += 1;
    }
    let stats = &mut b.stats;
    stats.add("entries", b.entries.len() as u64);
    stats.add("distinct_words", words.len() as u64);
    stats.add("single_chars", words.iter().filter(|w| char_len(w) == 1).count() as u64);
    stats.add("distinct_syllables", syllables.len() as u64);
    stats.add("words_with_multiple_readings", per_word.values().filter(|&&n| n > 1).count() as u64);
    stats.add("rejected", b.rejected.len() as u64);
    let mut tags = Counter::new();
    for tag in &b.entries.values {
        tags.add(tag, 1);
    }
    for (tag, n) in tags.iter() {
        stats.add(&format!("tag_{tag}"), *n);
    }
    // Python counts these over a set, in no fixed order; here shortest first.
    let mut lengths = [0u64; 6];
    for w in &words {
        lengths[char_len(w).min(5)] += 1;
    }
    for (n, &count) in lengths.iter().enumerate().filter(|(_, &c)| c > 0) {
        stats.add(&format!("len_{n}"), count);
    }
    let body: Vec<String> = stats.iter().map(|(k, v)| format!(" {}: {v}", serde_json::Value::String(k.clone()))).collect();
    fs::write(out_dir.join("stats.json"), format!("{{\n{}\n}}", body.join(",\n")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_text_rules() {
        assert_eq!(split_syllables(" ㄏㄨㄚˋㄦ\u{3000}ㄧ "), ["ㄏㄨㄚˋ", "ㄦˊ", "ㄧ"]);
        assert_eq!(split_syllables("ㄦ"), ["ㄦ"]);
        assert_eq!(
            readings_from(Some("ㄧˋ（變）ㄧˊ")),
            vec![vec!["ㄧˋ".to_owned()], vec!["ㄧˊ".to_owned()]]
        );
        assert_eq!(clean_word(Some(" 行(1) ")), "行");
        assert_eq!(clean_word(Some("一二三12")), "一二三");
        assert_eq!(clean_word(Some("(未完")), "(未完");
        assert!(is_syllable("˙ㄌㄜ") && is_syllable("ㄓㄨㄤˋ") && !is_syllable("˙") && !is_syllable("ㄅㄆㄇㄈ"));
        assert!(is_cjk_word("臺灣") && !is_cjk_word("A台") && !is_cjk_word(""));
    }

    #[test]
    fn products_in_python_order() {
        let o = vec![vec!["a".to_owned(), "b".to_owned()], vec!["x".to_owned(), "y".to_owned(), "z".to_owned()]];
        let got: Vec<String> = product(&o, 6).iter().map(|v| v.concat()).collect();
        assert_eq!(got, ["ax", "ay", "az", "bx", "by", "bz"]);
        assert_eq!(product(&o, 4).len(), 4);
    }

    #[test]
    fn sandhi() {
        assert_eq!(spoken('一', "ㄧ", "ㄍㄜˋ"), "ㄧˊ");
        assert_eq!(spoken('一', "ㄧ", "ㄑㄧˇ"), "ㄧˋ");
        assert_eq!(spoken('不', "ㄅㄨˋ", "ㄧㄠˋ"), "ㄅㄨˊ");
        assert_eq!(spoken('一', "ㄧ", "˙ㄌㄜ"), "ㄧ");
    }
}
