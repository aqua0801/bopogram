//! Test-set evaluation: top-1 accuracy, baselines, and per-syllable latency
//! (`prototype/bopogram/evaluate.py`; the same scoring and the same output).
//!
//! Systems:
//!   bopogram      the full engine (lattice + n-gram mixture + left context)
//!   word-unigram  same lattice, but words are scored without any context
//!   char-unigram  each syllable becomes its most frequent character
//!
//! Latency is measured the way an input method is used: after every syllable
//! the lattice is extended (and re-ranked) and the best result is read back.
//!
//! Scoring: where the output uses the other standard form of a character or
//! word (臺/台, 記錄/紀錄, ...), it counts as right.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use crate::context::context_state;
use crate::decoder::{Decoder, Style};
use crate::engine::Engine;

const OVERLAP_MARK: &str = "〔語料重複〕";
const VARIANT_CHARS: [&str; 12] = ["臺台", "裏裡", "祕秘", "週周", "佈布", "姊姐", "佔占", "羣群", "汙污", "菸煙", "豔艷", "歎嘆"];
const VARIANT_WORDS: [(&str, &str); 9] = [
    ("記錄", "紀錄"),
    ("作法", "做法"),
    ("作夢", "做夢"),
    ("作為", "做為"),
    ("身分", "身份"),
    ("部分", "部份"),
    ("甚麼", "什麼"),
    ("計畫", "計劃"),
    ("念書", "唸書"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum System {
    Bopogram,
    WordUnigram,
    CharUnigram,
}

impl System {
    pub fn parse(name: &str) -> Option<System> {
        match name {
            "bopogram" => Some(System::Bopogram),
            "word-unigram" => Some(System::WordUnigram),
            "char-unigram" => Some(System::CharUnigram),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            System::Bopogram => "bopogram",
            System::WordUnigram => "word-unigram",
            System::CharUnigram => "char-unigram",
        }
    }
}

fn variant_char(a: char, b: char) -> bool {
    VARIANT_CHARS.iter().any(|p| {
        let mut it = p.chars();
        let (x, y) = (it.next().unwrap(), it.next().unwrap());
        (a, b) == (x, y) || (a, b) == (y, x)
    })
}

fn variant_word(a: &[char], b: &[char]) -> bool {
    if a == b {
        // frozenset((w, w)) has one element and matches no pair.
        return false;
    }
    let a: String = a.iter().collect();
    let b: String = b.iter().collect();
    VARIANT_WORDS.iter().any(|&(x, y)| (a == x && b == y) || (a == y && b == x))
}

/// `gold` with every character where `hyp` uses the other standard form
/// replaced by `hyp`'s, so that such a difference does not count as an error.
pub fn accept_variants(gold: &[char], hyp: &[char]) -> Vec<char> {
    if gold.len() != hyp.len() {
        return gold.to_vec();
    }
    let mut out = gold.to_vec();
    for i in 0..gold.len() {
        let (g, h) = (gold[i], hyp[i]);
        if g == h {
            continue;
        }
        let word = |k: usize| {
            let end = (k + 2).min(gold.len());
            variant_word(&gold[k..end], &hyp[k..end])
        };
        if variant_char(g, h) || (i >= 1 && word(i - 1)) || word(i) {
            out[i] = h;
        }
    }
    out
}

fn merge_pronouns(text: &[char]) -> Vec<char> {
    text.iter().map(|&c| if matches!(c, '她' | '它' | '牠' | '祂') { '他' } else { c }).collect()
}

/// A tab-separated file with a header, read as Python's csv.DictReader reads
/// it (QUOTE_NONE): missing fields are absent, empty rows are skipped.
fn load_table(path: &Path) -> io::Result<Vec<HashMap<String, String>>> {
    let text = std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let header: Vec<String> = lines.next().unwrap_or("").split('\t').map(str::to_owned).collect();
    let mut rows = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        rows.push(header.iter().cloned().zip(line.split('\t').map(str::to_owned)).collect());
    }
    Ok(rows)
}

/// id -> syllables. ASCII spaces only: a literal full-width space is an item.
fn load_readings(path: &Path) -> io::Result<HashMap<String, Vec<String>>> {
    let text = std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    let mut out = HashMap::new();
    for line in text.split_inclusive('\n') {
        let parts: Vec<&str> = line.strip_suffix('\n').unwrap_or(line).split('\t').collect();
        if parts.len() >= 2 && parts[0] != "id" {
            out.insert(parts[0].to_owned(), parts[1].split(' ').filter(|p| !p.is_empty()).map(str::to_owned).collect());
        }
    }
    Ok(out)
}

/// Python's round(): halves to even.
fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        2.0 * (x / 2.0).round()
    } else {
        r
    }
}

pub fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let k = round_half_even(q * (v.len() - 1) as f64).max(0.0) as usize;
    v[k.min(v.len() - 1)]
}

/// Python's round(x, n), via the correctly rounded decimal text.
fn round_to(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse().unwrap_or(x)
}

#[derive(Default)]
struct Group {
    sentences: f64,
    correct: f64,
    correct_pron: f64,
    chars: f64,
    chars_correct: f64,
}

pub struct Options<'a> {
    pub testset: &'a Path,
    pub readings: &'a Path,
    pub system: System,
    pub char_freq: Option<&'a Path>,
    pub style: Style,
    pub errors: Option<&'a Path>,
    pub json: Option<&'a Path>,
}

/// Evaluate `engine` (loaded without personal data) and print the report.
pub fn run(engine: &mut Engine, opts: &Options, out: &mut dyn io::Write) -> io::Result<()> {
    let rows = load_table(opts.testset)?;
    let readings = load_readings(opts.readings)?;
    engine.set_style(opts.style);
    let system = opts.system;
    let live = engine.charlm.is_some() && system == System::Bopogram;

    let mut char_freq: HashMap<char, f64> = HashMap::new();
    if system == System::CharUnigram {
        let path = opts
            .char_freq
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--char-freq is required for the char-unigram baseline"))?;
        let text = std::fs::read_to_string(path)?;
        for line in text.split_inclusive('\n') {
            let line = line.strip_suffix('\n').unwrap_or(line);
            let (ch, n) = line.split_once('\t').unwrap_or((line, ""));
            let mut it = ch.chars();
            let n: f64 = n.trim().parse().map_err(|_| io::Error::other(format!("{}: bad count {n:?}", path.display())))?;
            // Python keys by the whole field; only single characters can match.
            if let (Some(c), None) = (it.next(), it.next()) {
                char_freq.insert(c, n);
            }
        }
    }
    let mut unigram_decoder = None;
    if system == System::WordUnigram {
        engine.lm.set_context_free();
        unigram_decoder = Some(Decoder::new(
            Rc::clone(&engine.lexicon),
            Rc::clone(&engine.lm),
            1,
            engine.max_candidates(),
            None,
            0.0,
            0.0,
            opts.style,
            Rc::new(HashMap::new()),
        ));
    }

    let mut convert = |engine: &mut Engine, syls: &[String], context: &str, timings: &mut Vec<f64>| -> String {
        if system == System::CharUnigram {
            let lexicon = engine.lexicon.borrow();
            let mut text = String::new();
            for syl in syls {
                let t0 = Instant::now();
                let mut best: Option<(char, f64)> = None;
                for w in lexicon.candidates(&[syl.as_str()]) {
                    let mut it = w.chars();
                    if let (Some(c), None) = (it.next(), it.next()) {
                        let f = char_freq.get(&c).copied().unwrap_or(0.0);
                        if best.is_none_or(|(_, b)| f > b) {
                            best = Some((c, f));
                        }
                    }
                }
                text.push(best.map_or('?', |b| b.0));
                timings.push(t0.elapsed().as_secs_f64());
            }
            return text;
        }
        let charlm = engine.charlm.clone();
        let decoder = match unigram_decoder.as_mut() {
            Some(d) => {
                d.reset(None);
                d
            }
            None => {
                let state = context_state(context, &engine.lexicon.borrow(), &engine.lm);
                engine.decoder.reset(Some(state));
                &mut engine.decoder
            }
        };
        let mut text = String::new();
        for syl in syls {
            let t0 = Instant::now();
            decoder.push(syl.clone());
            if live {
                if let Some(c) = &charlm {
                    c.rescore(decoder, context);
                }
            }
            text = decoder.best_words().iter().map(|w| &**w).collect();
            timings.push(t0.elapsed().as_secs_f64());
        }
        text
    };

    let field = |row: &HashMap<String, String>, name: &str| row.get(name).cloned().unwrap_or_default();
    // Warm up caches so the first sentences do not distort latency.
    for row in rows.iter().take(20) {
        if let Some(syls) = readings.get(&field(row, "id")) {
            convert(engine, syls, &field(row, "context"), &mut Vec::new());
        }
    }

    let mut stats: BTreeMap<String, Group> = BTreeMap::new();
    let mut timings = Vec::new();
    let mut errors = Vec::new();
    let mut skipped = 0;
    for row in &rows {
        let id = field(row, "id");
        let gold_text = field(row, "text");
        let mut gold: Vec<char> = gold_text.chars().collect();
        let syls = match readings.get(&id) {
            Some(s) if !s.is_empty() && s.len() == gold.len() => s,
            _ => {
                skipped += 1;
                continue;
            }
        };
        let context = field(row, "context");
        let hyp: Vec<char> = convert(engine, syls, &context, &mut timings).chars().collect();
        if opts.style == Style::Casual {
            // Casual style writes 的 for every structural particle; that is not an error.
            gold = gold
                .iter()
                .zip(&hyp)
                .zip(syls)
                .map(|((&g, &h), s)| if s == "˙ㄉㄜ" && (g == '地' || g == '得') && h == '的' { '的' } else { g })
                .collect();
        }
        let gold = accept_variants(&gold, &hyp);
        let correct_chars =
            if hyp.len() == gold.len() { hyp.iter().zip(&gold).filter(|(a, b)| a == b).count() } else { 0 };
        let category = field(row, "category");
        let note = field(row, "note");
        let mut groups = vec!["all".to_owned(), category.clone()];
        if !note.contains(OVERLAP_MARK) {
            groups.push("all_excluding_overlap".to_owned());
        }
        for g in groups {
            let s = stats.entry(g).or_default();
            s.sentences += 1.0;
            s.correct += (hyp == gold) as u8 as f64;
            s.correct_pron += (merge_pronouns(&hyp) == merge_pronouns(&gold)) as u8 as f64;
            s.chars += gold.len() as f64;
            s.chars_correct += correct_chars as f64;
        }
        if hyp != gold {
            let gold: String = gold.iter().collect();
            let hyp: String = hyp.iter().collect();
            errors.push(format!("{id}\t{category}\t{gold}\t{hyp}\t{note}"));
        }
    }

    let mut groups: Vec<(&String, &Group)> = stats.iter().collect();
    groups.sort_by_key(|(k, _)| (!k.starts_with("all"), (*k).clone()));
    writeln!(out, "system: {}   (skipped {skipped} rows without usable readings)", system.name())?;
    writeln!(out, "{:<24}{:>10}{:>10}{:>10}{:>28}", "group", "sentences", "sent.acc", "char.acc", "sent.acc(pronouns merged)")?;
    let pct = |x: f64| format!("{:.1}%", x * 100.0);
    let mut json_groups = Vec::new();
    for (g, s) in &groups {
        let (sent, chars, pron) = (s.correct / s.sentences, s.chars_correct / s.chars, s.correct_pron / s.sentences);
        writeln!(out, "{g:<24}{:>10}{:>10}{:>10}{:>28}", s.sentences as u64, pct(sent), pct(chars), pct(pron))?;
        json_groups.push(format!(
            "  {}: {{\n   \"sentences\": {},\n   \"sentence_accuracy\": {},\n   \"char_accuracy\": {},\n   \"sentence_accuracy_pronouns_merged\": {}\n  }}",
            serde_json::Value::String((*g).clone()),
            s.sentences as u64,
            json_float(round_to(sent, 4)),
            json_float(round_to(chars, 4)),
            json_float(round_to(pron, 4)),
        ));
    }
    let ms: Vec<f64> = timings.iter().map(|t| t * 1000.0).collect();
    let (p50, p95) = (round_to(percentile(&ms, 0.5), 3), round_to(percentile(&ms, 0.95), 3));
    let mean = if ms.is_empty() { 0.0 } else { round_to(ms.iter().sum::<f64>() / ms.len() as f64, 3) };
    writeln!(out, "latency per syllable: P50 {p50:.2} ms, P95 {p95:.2} ms over {} syllables", ms.len())?;
    if let Some(path) = opts.errors {
        let mut text = String::from("id\tcategory\tgold\toutput\tnote\n");
        text.push_str(&errors.join("\n"));
        text.push('\n');
        std::fs::write(path, text)?;
    }
    if let Some(path) = opts.json {
        let mut text = String::new();
        let _ = write!(
            text,
            "{{\n \"system\": \"{}\",\n \"skipped\": {skipped},\n \"groups\": {{\n{}\n }},\n \"latency_ms\": {{\n  \"p50\": {},\n  \"p95\": {},\n  \"mean\": {},\n  \"keystrokes\": {}\n }}\n}}",
            system.name(),
            json_groups.join(",\n"),
            json_float(p50),
            json_float(p95),
            json_float(mean),
            ms.len()
        );
        std::fs::write(path, text)?;
    }
    Ok(())
}

/// A float as Python's json writes it (shortest form, always with a point).
fn json_float(x: f64) -> String {
    let s = format!("{x:?}");
    if s.contains(['.', 'e', 'i', 'N']) {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn variants_and_rounding() {
        let got: String = accept_variants(&chars("我的紀錄在臺北"), &chars("我的記錄在台北")).iter().collect();
        assert_eq!(got, "我的記錄在台北");
        let got: String = accept_variants(&chars("我在家"), &chars("我再家")).iter().collect();
        assert_eq!(got, "我在家");
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.5), 3.0); // round(1.5) = 2
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 0.5), 3.0); // round(2.5) = 2
        assert_eq!(json_float(93.0), "93.0");
        assert_eq!(json_float(0.932), "0.932");
    }
}
