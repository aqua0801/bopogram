//! Command line, the same as the Python prototype's (`prototype/bopogram/cli.py`):
//!
//!     bopogram convert --keys "ji3y94ru8 "
//!     bopogram convert --bopomofo "ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ" --context "上文" --nbest 5
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

const USAGE: &str = "usage: bopogram [--config FILE] [--models DIR] {convert|lookup|serve|build-lm} ...";

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
        "serve" => serve(&mut args, config.as_deref(), models.as_deref()),
        "build-lm" => build_lm(&mut args),
        "lm-probe" => lm_probe(&mut args),
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
