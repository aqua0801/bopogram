//! Command line, the same as the Python prototype's (`prototype/bopogram/cli.py`):
//!
//!     bopogram convert --keys "ji3y94ru8 "
//!     bopogram convert --bopomofo "ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ" --context "上文" --nbest 5
//!     bopogram eval eval/testset/testset.tsv --readings eval/readings/testset_bopomofo.tsv
//!     bopogram lookup 垃圾 行
//!     bopogram lookup --bopomofo "ㄌㄜˋ ㄙㄜˋ"
//!     bopogram serve [--trace FILE]    # JSON lines on stdin/stdout, see server.rs
//!
//! and, for this engine only:
//!
//!     bopogram build-lm MODEL.arpa MODEL.bglm     # model file from an ARPA file
//!     bopogram convert --batch < lines           # context <Tab> syllables per line; best result per line

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use bopogram::decoder::Style;
use bopogram::engine::{config_path, load_lexicon, read_config, Engine};
use bopogram::layout::layout;
use bopogram::lm::build::build;

const USAGE: &str = "usage: bopogram [--config FILE] [--models DIR] {convert|eval|lookup|serve|build-lm} ...";

struct Args {
    rest: Vec<String>,
}

impl Args {
    /// The value after `--name`, removed from the arguments.
    fn take(&mut self, name: &str) -> Option<String> {
        let i = self.rest.iter().position(|a| a == name)?;
        if i + 1 >= self.rest.len() {
            self.rest.remove(i);
            return Some(String::new());
        }
        let value = self.rest.remove(i + 1);
        self.rest.remove(i);
        Some(value)
    }

    fn flag(&mut self, name: &str) -> bool {
        match self.rest.iter().position(|a| a == name) {
            Some(i) => {
                self.rest.remove(i);
                true
            }
            None => false,
        }
    }
}

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args = Args { rest: std::env::args().skip(1).collect() };
    let config = args.take("--config");
    // Folder of our model files (*.bglm), when not beside the config's models.
    let models = args.take("--models").map(std::path::PathBuf::from);
    if args.rest.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let command = args.rest.remove(0);
    let result = match command.as_str() {
        "convert" => convert(&mut args, config.as_deref(), models.as_deref()),
        "lookup" => lookup(&mut args, config.as_deref()),
        "eval" => eval(&mut args, config.as_deref(), models.as_deref()),
        "serve" => serve(&mut args, config.as_deref(), models.as_deref()),
        "build-lm" => build_lm(&mut args),
        "lm-probe" => lm_probe(&mut args),
        "charlm-check" => charlm_check(&mut args),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(code) => code,
        Err(err) => fail(err),
    }
}

fn convert(args: &mut Args, config: Option<&str>, models: Option<&Path>) -> io::Result<ExitCode> {
    let keys = args.take("--keys");
    let bopomofo = args.take("--bopomofo");
    let batch = args.flag("--batch");
    let layout_name = args.take("--layout").unwrap_or_else(|| "dachen".into());
    let context = args.take("--context").unwrap_or_default();
    let nbest: usize = args.take("--nbest").map_or(Ok(1), |n| n.parse()).map_err(|_| io::Error::other("--nbest: not a number"))?;
    let style = args.take("--style");
    let Some(layout) = layout(&layout_name) else {
        return Ok(fail(format!("unknown layout {layout_name:?}")));
    };
    let syllables: Vec<String> = match (&keys, &bopomofo, batch) {
        (Some(k), None, false) => match layout.to_syllables(k) {
            Ok(s) => s,
            Err(e) => return Ok(fail(e)),
        },
        (None, Some(b), false) => b.split_whitespace().map(str::to_owned).collect(),
        (None, None, true) => Vec::new(),
        _ => return Ok(fail("give one of --keys, --bopomofo, --batch")),
    };
    let started = Instant::now();
    let mut engine = Engine::load(&config_path(config), false, models)?;
    if let Some(style) = style {
        match Style::parse(&style) {
            Some(s) => engine.set_style(s),
            None => return Ok(fail(format!("unknown style {style:?}"))),
        }
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if batch {
        eprintln!("loaded in {:.1} s, models {} MB", started.elapsed().as_secs_f64(), engine.lm.memory() >> 20);
        for line in io::stdin().lock().lines() {
            let line = line?;
            let (context, syls) = line.split_once('\t').unwrap_or(("", line.as_str()));
            let syls: Vec<String> = syls.split_whitespace().map(str::to_owned).collect();
            let best = engine.convert(&syls, context, 1).into_iter().next().map(|r| r.0).unwrap_or_default();
            writeln!(out, "{best}")?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    for (text, score) in engine.convert(&syllables, &context, nbest) {
        if nbest > 1 {
            writeln!(out, "{text}\t{score:.3}")?;
        } else {
            writeln!(out, "{text}")?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn eval(args: &mut Args, config: Option<&str>, models: Option<&Path>) -> io::Result<ExitCode> {
    use bopogram::eval::{run, Options, System};
    for name in ["--neural-model", "--neural-scores", "--neural-mode", "--neural-weight", "--neural-floor", "--dump-nbest", "--beam", "--nbest"] {
        if args.rest.iter().any(|a| a == name) {
            return Ok(fail(format!("{name}: not in this engine (the neural model is not ported); use the prototype")));
        }
    }
    let readings = args.take("--readings");
    let system_name = args.take("--system").unwrap_or_else(|| "bopogram".into());
    let char_freq = args.take("--char-freq");
    let style_name = args.take("--style").unwrap_or_else(|| "formal".into());
    let charlm_model = args.take("--charlm-model");
    let parse_f64 = |v: Option<String>, name: &str, default: f64| -> Result<f64, String> {
        v.map_or(Ok(default), |v| v.parse().map_err(|_| format!("{name}: not a number")))
    };
    let charlm_weight = parse_f64(args.take("--charlm-weight"), "--charlm-weight", 0.5);
    let charlm_top = args.take("--charlm-top").map_or(Ok(5), |v| v.parse::<usize>().map_err(|_| "--charlm-top: not a number".to_owned()));
    let no_charlm = args.flag("--no-charlm");
    let errors = args.take("--errors");
    let json = args.take("--json");
    let (Ok(charlm_weight), Ok(charlm_top)) = (charlm_weight, charlm_top) else {
        return Ok(fail("--charlm-weight / --charlm-top: not a number"));
    };
    let [testset] = args.rest.as_slice() else {
        return Ok(fail("usage: bopogram eval TESTSET --readings READINGS [--system bopogram|word-unigram|char-unigram] [--style casual|formal] ..."));
    };
    let Some(readings) = readings else { return Ok(fail("--readings is required")) };
    let Some(system) = System::parse(&system_name) else { return Ok(fail(format!("unknown system {system_name:?}"))) };
    let Some(style) = Style::parse(&style_name) else { return Ok(fail(format!("unknown style {style_name:?}"))) };
    let mut engine = Engine::load(&config_path(config), false, models)?;
    if let Some(model) = charlm_model {
        match bopogram::engine::load_rerank(Path::new(&model), models, charlm_weight, charlm_top)? {
            Some(r) => engine.set_charlm(Some(r)),
            None => return Ok(fail(format!("{model}: no charlm.bin for it"))),
        }
    }
    if no_charlm {
        engine.set_charlm(None);
    }
    let (testset, readings) = (Path::new(testset), Path::new(&readings));
    let opts = Options {
        testset,
        readings,
        system,
        char_freq: char_freq.as_deref().map(Path::new),
        style,
        errors: errors.as_deref().map(Path::new),
        json: json.as_deref().map(Path::new),
    };
    run(&mut engine, &opts, &mut io::stdout().lock())?;
    Ok(ExitCode::SUCCESS)
}

fn lookup(args: &mut Args, config: Option<&str>) -> io::Result<ExitCode> {
    let bopomofo = args.take("--bopomofo");
    let layout_name = args.take("--layout").unwrap_or_else(|| "dachen".into());
    let Some(layout) = layout(&layout_name) else {
        return Ok(fail(format!("unknown layout {layout_name:?}")));
    };
    let path = config_path(config);
    let cfg = read_config(&path)?;
    let lexicon = load_lexicon(&cfg, path.parent().unwrap_or(Path::new(".")))?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Some(b) = bopomofo {
        let syls: Vec<&str> = b.split_whitespace().collect();
        let words = lexicon.candidates(&syls);
        if words.is_empty() {
            writeln!(out, "(the dictionary has no word with this reading)")?;
        } else {
            writeln!(out, "{}", words.iter().map(|w| &**w).collect::<Vec<_>>().join(" "))?;
        }
    }
    let show = |out: &mut dyn Write, word: &str, indent: &str| -> io::Result<bool> {
        let readings = lexicon.readings(word);
        for (syls, tag) in &readings {
            let note = match tag.split_once(':').map(|(_, t)| t) {
                Some("var") => "  (variant reading)",
                Some("alt") => "  (citation-tone spelling)",
                _ => "",
            };
            writeln!(out, "{indent}{word}\t{}\tkeys: {:?}{note}", syls.join(" "), layout.to_keys(syls))?;
        }
        Ok(!readings.is_empty())
    };
    for word in args.rest.clone() {
        if !show(&mut out, &word, "")? {
            writeln!(out, "{word}\t(not in the dictionary as a word)")?;
        }
        if word.chars().count() > 1 {
            let mut seen = Vec::new();
            for ch in word.chars() {
                if seen.contains(&ch) {
                    continue;
                }
                seen.push(ch);
                let ch = ch.to_string();
                if !show(&mut out, &ch, "  ")? {
                    writeln!(out, "  {ch}\t(not in the dictionary: this character cannot be typed)")?;
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn serve(args: &mut Args, config: Option<&str>, models: Option<&Path>) -> io::Result<ExitCode> {
    let layout_name = args.take("--layout").unwrap_or_else(|| "dachen".into());
    let trace = args.take("--trace");
    let Some(layout) = layout(&layout_name) else {
        return Ok(fail(format!("unknown layout {layout_name:?}")));
    };
    let engine = std::rc::Rc::new(Engine::load(&config_path(config), true, models)?);
    let trace: Option<Box<dyn Write>> = match trace {
        Some(path) => Some(Box::new(std::fs::OpenOptions::new().create(true).append(true).open(path)?)),
        None => None,
    };
    bopogram::server::serve(engine, layout, io::stdin().lock(), io::stdout().lock(), trace)?;
    Ok(ExitCode::SUCCESS)
}

/// Development: scores and state classes for token sequences, to compare a
/// model file with the KenLM binary it came from. Each input line is `S` or
/// `N` (start of sentence or no context), a Tab, and tokens separated by spaces;
/// each output line has `score:class` per token, the class numbering the
/// distinct states in the order they first appear.
fn lm_probe(args: &mut Args) -> io::Result<ExitCode> {
    use bopogram::lm::{Model, State};
    let [model, seqs, out] = args.rest.as_slice() else {
        return Ok(fail("usage: bopogram lm-probe MODEL.bglm SEQUENCES OUT"));
    };
    let model = Model::load(Path::new(model))?;
    let begin = model.id("<s>").unwrap_or(0);
    let mut classes: std::collections::HashMap<State, usize> = std::collections::HashMap::new();
    let mut out = io::BufWriter::new(std::fs::File::create(out)?);
    for line in std::fs::read_to_string(seqs)?.lines() {
        let (start, words) = line.split_once('\t').unwrap_or(("N", line));
        let mut state = if start == "S" { model.begin_sentence(begin) } else { State::EMPTY };
        let mut parts = Vec::new();
        for w in words.split(' ') {
            let (p, next) = model.score(&state, model.id(w).unwrap_or(0));
            let n = classes.len();
            let class = *classes.entry(next).or_insert(n);
            parts.push(format!("{:?}:{class}", p as f64));
            state = next;
        }
        writeln!(out, "{}", parts.join(" "))?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Development: check the character model against the reference's test
/// vectors (`prototype/tools/charlm_vectors.py`): the encoding, and the scores
/// computed from scratch, which must be the same to the last bit; then the
/// same scores again through the prefix cache, timed.
fn charlm_check(args: &mut Args) -> io::Result<ExitCode> {
    use bopogram::charlm::CharLM;
    let [model, vectors] = args.rest.as_slice() else {
        return Ok(fail("usage: bopogram charlm-check MODEL_DIR VECTORS.jsonl"));
    };
    let dir = Path::new(model);
    let mut lm = CharLM::load(&dir.join("charlm.bin"), &dir.join("charlm.json"))?;
    let (mut calls, mut seqs, mut bad_seq, mut exact, mut worst) = (0, 0, 0, 0, 0.0f64);
    let mut cached_bad = 0;
    let mut all = Vec::new();
    let started = Instant::now();
    for (n, line) in std::fs::read_to_string(vectors)?.lines().enumerate() {
        let rec: serde_json::Value = serde_json::from_str(line).map_err(io::Error::other)?;
        let context = rec["context"].as_str().unwrap_or("");
        let texts: Vec<String> = rec["texts"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_owned)).collect();
        calls += 1;
        for (i, text) in texts.iter().enumerate() {
            seqs += 1;
            let (seq, first) = lm.sequence(context, text);
            let want_seq: Vec<u32> = rec["seqs"][i].as_array().into_iter().flatten().filter_map(|v| v.as_u64().map(|v| v as u32)).collect();
            if seq != want_seq || Some(first as u64) != rec["firsts"][i].as_u64() {
                bad_seq += 1;
                eprintln!("line {}: encoding of {text:?} differs", n + 1);
                continue;
            }
            lm.model.clear_cache();
            let got = lm.model.logp(&seq, first);
            let want = rec["logp"][i].as_f64().unwrap_or(f64::NAN);
            if got.to_bits() == want.to_bits() {
                exact += 1;
            } else {
                eprintln!("line {}: {text:?} {got:?} != {want:?} ({:.3e})", n + 1, got - want);
            }
            worst = worst.max((got - want).abs());
            all.push((seq, first, want));
        }
    }
    let fresh = started.elapsed().as_secs_f64();
    lm.model.clear_cache();
    let started = Instant::now();
    for (seq, first, want) in &all {
        if lm.model.logp(seq, *first).to_bits() != want.to_bits() {
            cached_bad += 1;
        }
    }
    let cached = started.elapsed().as_secs_f64();
    println!("{calls} calls, {seqs} sequences: encoding differs {bad_seq}, scores exact {exact}, largest difference {worst:.3e}");
    println!("through the prefix cache: {cached_bad} differ from the reference");
    println!("from scratch {:.1} ms per sequence; with the cache {:.1} ms", fresh * 1e3 / seqs as f64, cached * 1e3 / all.len().max(1) as f64);
    Ok(ExitCode::SUCCESS)
}

fn build_lm(args: &mut Args) -> io::Result<ExitCode> {
    let [arpa, out] = args.rest.as_slice() else {
        return Ok(fail("usage: bopogram build-lm MODEL.arpa MODEL.bglm"));
    };
    let started = Instant::now();
    let report = build(Path::new(arpa), Path::new(out), &mut |message| {
        eprintln!("{:>6.1} s  {message}", started.elapsed().as_secs_f64());
    })?;
    eprintln!(
        "done in {:.1} s: n-grams {:?}; back-off without children {}, children without back-off {}",
        started.elapsed().as_secs_f64(),
        report.counts,
        report.extends_without_children,
        report.children_without_backoff
    );
    Ok(ExitCode::SUCCESS)
}
