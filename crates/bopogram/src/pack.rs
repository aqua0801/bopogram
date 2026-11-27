//! Model packs (`prototype/docs/release-spec.md`): files the user downloads
//! and hands to the settings program or the installer. Nothing here touches
//! the network.
//!
//! The data folder (ROOT) holds
//!
//!   models/            the pack in use, as unpacked, plus what is made here:
//!                      dict.tsv (built from the MOE tables in dict-source/)
//!                      and engine.json, the engine's configuration
//!   models-previous/   the pack before it, for going back
//!   userdata/          the user's data, never touched by an update
//!
//! `engine.json` is the pack's `config.json` (kept unchanged, so that its
//! checksum still holds) with the user's data in `../userdata/` and the
//! user's choices carried over from the engine.json before: learning, the
//! typing log, the user dictionary, the charlm tier.
//!
//! Installing is two steps. `prepare` checks the pack against its manifest,
//! unpacks it into `models-new/`, builds the dictionary and runs the pack's
//! self-test, all while the engine keeps running. `switch` then swaps the
//! folders, also while the engine runs (Rust opens files so that they can be
//! renamed, on Windows too): the engine keeps the old files, now in
//! models-previous, until it is restarted, which is the caller's last step.
//! Removing an older models-previous fails while an engine still uses it
//! (two updates without a restart); the error says so. `rollback` swaps back.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::decoder::Style;
use crate::engine::Engine;

pub const CURRENT: &str = "models";
pub const PREVIOUS: &str = "models-previous";
pub const NEW: &str = "models-new";
pub const ENGINE_CONFIG: &str = "engine.json";
const MANIFEST: &str = "manifest.json";
const MODELS_NAME: &str = "bopogram-models";
const CHARLM_NAME: &str = "bopogram-charlm";
/// The charlm tiers and their files; "standard" is the pack's own model.
pub const TIERS: [&str; 4] = ["off", "lite", "standard", "advanced"];

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// The version of this program, which a pack's `engine` field is compared with.
pub fn engine_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The data folder: `%LOCALAPPDATA%\Bopogram\data` on Windows, else
/// `$XDG_DATA_HOME/bopogram` or `~/.local/share/bopogram`.
pub fn default_root() -> PathBuf {
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        return env("LOCALAPPDATA").unwrap_or_default().join("Bopogram").join("data");
    }
    env("XDG_DATA_HOME")
        .map(|d| d.join("bopogram"))
        .unwrap_or_else(|| env("HOME").unwrap_or_else(|| PathBuf::from("/tmp")).join(".local/share/bopogram"))
}

fn version_parts(v: &str) -> Option<Vec<u64>> {
    v.trim().split('.').map(|p| p.parse().ok()).collect()
}

/// `>=0.2`: the only form a manifest uses.
fn satisfies(requirement: &str, version: &str) -> Result<bool, String> {
    let wanted = requirement.trim().strip_prefix(">=").ok_or_else(|| format!("unknown engine requirement {requirement:?}"))?;
    let (Some(mut want), Some(mut have)) = (version_parts(wanted), version_parts(version)) else {
        return Err(format!("unknown engine requirement {requirement:?}"));
    };
    let n = want.len().max(have.len());
    want.resize(n, 0);
    have.resize(n, 0);
    Ok(have >= want)
}

pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

pub struct Manifest {
    /// The file as it is in the pack (kept beside the unpacked files).
    pub text: String,
    pub name: String,
    pub version: String,
    pub engine: String,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    fn parse(text_all: &str) -> io::Result<Manifest> {
        let v: Value = serde_json::from_str(text_all).map_err(|e| bad(format!("manifest.json: {e}")))?;
        if v.get("format").and_then(Value::as_u64) != Some(1) {
            return Err(bad("manifest.json: unknown format (a newer pack? update the program first)"));
        }
        let text = |name: &str| v.get(name).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| bad(format!("manifest.json: no {name}")));
        let files = v
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("manifest.json: no files"))?
            .iter()
            .map(|f| {
                Some(FileEntry {
                    path: f.get("path")?.as_str()?.to_owned(),
                    size: f.get("size")?.as_u64()?,
                    sha256: f.get("sha256")?.as_str()?.to_ascii_lowercase(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| bad("manifest.json: a file without path, size or sha256"))?;
        Ok(Manifest { text: text_all.to_owned(), name: text("name")?, version: text("version")?, engine: text("engine")?, files })
    }
}

/// A path inside the pack: relative, no `..`, forward slashes.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    if path.contains(['\\', ':']) || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(p.to_path_buf())
}

struct Archive {
    zip: zip::ZipArchive<fs::File>,
    /// The folder every member is in ("bopogram-models-2026.12/").
    top: String,
}

impl Archive {
    fn open(path: &Path) -> io::Result<Archive> {
        let file = fs::File::open(path)?;
        let zip = zip::ZipArchive::new(file).map_err(|e| bad(format!("{}: not a zip file ({e})", path.display())))?;
        let manifest = zip
            .file_names()
            .filter_map(Result::ok)
            .find(|n| n.ends_with("/manifest.json") && n.matches('/').count() == 1)
            .map(|n| n.into_owned())
            .ok_or_else(|| bad("no manifest.json: not a Bopogram pack"))?;
        let top = manifest.trim_end_matches(MANIFEST).to_owned();
        Ok(Archive { zip, top })
    }

    fn manifest(&mut self) -> io::Result<Manifest> {
        let mut text = String::new();
        let name = format!("{}{MANIFEST}", self.top);
        self.zip.by_name(&name).map_err(|e| bad(e.to_string()))?.read_to_string(&mut text)?;
        Manifest::parse(&text)
    }
}

/// What a pack is, after its manifest has been checked.
pub struct Checked {
    pub manifest: Manifest,
    archive: Archive,
}

/// Open a pack and check its manifest: the kind, the program version it
/// needs, and that every member is listed (and nothing else is there).
pub fn check(zip_path: &Path) -> io::Result<Checked> {
    let mut archive = Archive::open(zip_path)?;
    let manifest = archive.manifest()?;
    if manifest.name != MODELS_NAME && manifest.name != CHARLM_NAME {
        return Err(bad(format!("not a Bopogram pack ({:?})", manifest.name)));
    }
    match satisfies(&manifest.engine, engine_version()) {
        Ok(true) => {}
        Ok(false) => {
            return Err(bad(format!("this pack needs program version {}; this is {}", manifest.engine, engine_version())))
        }
        Err(e) => return Err(bad(e)),
    }
    for f in &manifest.files {
        if safe_relative(&f.path).is_none() {
            return Err(bad(format!("manifest.json: bad path {:?}", f.path)));
        }
    }
    let listed: std::collections::HashSet<String> = manifest.files.iter().map(|f| format!("{}{}", archive.top, f.path)).collect();
    let names: Vec<String> = archive.zip.file_names().filter_map(Result::ok).map(|n| n.into_owned()).collect();
    for name in &names {
        let is_dir = name.ends_with('/');
        if !is_dir && *name != format!("{}{MANIFEST}", archive.top) && !listed.contains(name) {
            return Err(bad(format!("{name}: in the pack but not in its manifest")));
        }
    }
    for name in &listed {
        if !names.contains(name) {
            return Err(bad(format!("{name}: in the manifest but not in the pack")));
        }
    }
    Ok(Checked { manifest, archive })
}

/// Unpack every listed file into `dest`, checking its size and SHA-256.
fn unpack(checked: &mut Checked, dest: &Path, progress: &mut dyn Write) -> io::Result<()> {
    let total: u64 = checked.manifest.files.iter().map(|f| f.size).sum();
    let mut done = 0u64;
    for f in &checked.manifest.files {
        let target = dest.join(safe_relative(&f.path).expect("checked"));
        fs::create_dir_all(target.parent().unwrap_or(dest))?;
        let name = format!("{}{}", checked.archive.top, f.path);
        let mut member = checked.archive.zip.by_name(&name).map_err(|e| bad(format!("{name}: {e}")))?;
        let mut out = io::BufWriter::new(fs::File::create(&target)?);
        let mut hash = Sha256::new();
        let mut size = 0u64;
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let n = member.read(&mut buffer).map_err(|e| bad(format!("{}: damaged ({e})", f.path)))?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            out.write_all(&buffer[..n])?;
            size += n as u64;
        }
        out.flush()?;
        let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if size != f.size || digest != f.sha256 {
            return Err(bad(format!("{}: does not match the manifest (damaged or changed download)", f.path)));
        }
        done += size;
        let _ = writeln!(progress, "{:3}% {}", done * 100 / total.max(1), f.path);
    }
    Ok(())
}

fn read_json(path: &Path) -> io::Result<Value> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", path.display())))
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(value).map_err(io::Error::other)? + "\n")?;
    fs::rename(&tmp, path)
}

/// The charlm model of a tier in a pack folder, if the pack has it.
fn tier_model(pack: &Path, config: &Value, tier: &str) -> Option<String> {
    let standard = config.get("charlm")?.get("model")?.as_str()?.to_owned();
    let model = match tier {
        "standard" => standard,
        "lite" | "advanced" => {
            let dir = Path::new(&standard).parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
            format!("{prefix}charlm-{tier}.bin")
        }
        _ => return None,
    };
    pack.join(&model).is_file().then_some(model)
}

/// The tier an engine.json uses.
pub fn tier_of(engine: &Value) -> &'static str {
    match engine.get("charlm") {
        Some(Value::Null) => "off",
        Some(c) => {
            let model = c.get("model").and_then(Value::as_str).unwrap_or("");
            if model.ends_with("charlm-lite.bin") {
                "lite"
            } else if model.ends_with("charlm-advanced.bin") {
                "advanced"
            } else {
                "standard"
            }
        }
        None => "off",
    }
}

/// Keys of engine.json that hold the user's choices.
const USER_KEYS: [&str; 4] = ["user_model", "typing_log", "user_dict", "excluded_words"];

/// engine.json for the pack in `pack`: its config with the user's data in
/// `../userdata/`, the choices in `old` (the engine.json before) carried
/// over, and the charlm `tier`. Returns it and whether the tier had to fall
/// back to the standard one (the pack does not have it).
fn engine_config(pack: &Path, old: Option<&Value>, tier: &str) -> io::Result<(Value, bool)> {
    let config = read_json(&pack.join("config.json"))?;
    let mut engine = config.clone();
    let user = |name: &str, default: Option<&str>| -> Value {
        let path = match config.get(name) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.as_str()),
            Some(_) => None,
            None => default,
        };
        // "userdata/user.json" -> "../userdata/user.json"
        path.map_or(Value::Null, |p| Value::String(format!("../{p}")))
    };
    engine["user_model"] = user("user_model", None);
    engine["typing_log"] = Value::Null;
    engine["user_dict"] = user("user_dict", Some("userdata/userdict.tsv"));
    engine["excluded_words"] = user("excluded_words", Some("userdata/excluded.tsv"));
    if let Some(old) = old {
        for key in USER_KEYS {
            if let Some(v) = old.get(key) {
                engine[key] = v.clone();
            }
        }
    }
    let mut fell_back = false;
    if tier == "off" {
        engine["charlm"] = Value::Null;
    } else if let Some(model) = tier_model(pack, &config, tier) {
        engine["charlm"]["model"] = Value::String(model);
    } else {
        fell_back = tier != "standard";
        if let Some(model) = tier_model(pack, &config, "standard") {
            engine["charlm"]["model"] = Value::String(model);
        }
    }
    Ok((engine, fell_back))
}

/// The pack's self-test: convert its cases with the new models (no user
/// data) and get exactly what it expects.
fn self_test(pack: &Path, progress: &mut dyn Write) -> io::Result<()> {
    let test_file = pack.join("selftest.json");
    if !test_file.is_file() {
        return Ok(());
    }
    let cases = read_json(&test_file)?;
    let cases = cases.get("cases").and_then(Value::as_array).cloned().unwrap_or_default();
    // The pack's own configuration, as the expected results were made.
    let mut engine = Engine::load(&pack.join("config.json"), false, None)?;
    let mut failed = Vec::new();
    for case in &cases {
        let text = |name: &str| case.get(name).and_then(Value::as_str).unwrap_or("");
        let style = Style::parse(text("style")).ok_or_else(|| bad("selftest.json: bad style"))?;
        engine.set_style(style);
        let syllables: Vec<String> = text("bopomofo").split_whitespace().map(str::to_owned).collect();
        let got = engine.convert(&syllables, text("context"), 1).into_iter().next().map(|r| r.0).unwrap_or_default();
        if got != text("expect") {
            failed.push(format!("{} -> {got} (expected {})", text("bopomofo"), text("expect")));
        }
    }
    let _ = writeln!(progress, "self-test: {} of {} as expected", cases.len() - failed.len(), cases.len());
    if !failed.is_empty() {
        return Err(bad(format!("self-test failed:\n{}", failed.join("\n"))));
    }
    Ok(())
}

pub struct Prepared {
    pub version: String,
    /// The user's charlm tier is not in the new pack: the standard one is used.
    pub tier_fell_back: bool,
}

fn current_engine(root: &Path) -> Option<Value> {
    read_json(&root.join(CURRENT).join(ENGINE_CONFIG)).ok()
}

/// Check and unpack a model pack into ROOT/models-new, build its dictionary,
/// write its engine.json and run its self-test. The engine keeps running.
pub fn prepare(zip_path: &Path, root: &Path, progress: &mut dyn Write) -> io::Result<Prepared> {
    let mut checked = check(zip_path)?;
    if checked.manifest.name != MODELS_NAME {
        return Err(bad("this is a charlm tier pack: use `pack add-tier`"));
    }
    let new = root.join(NEW);
    if new.exists() {
        fs::remove_dir_all(&new)?;
    }
    fs::create_dir_all(&new)?;
    let result = (|| {
        unpack(&mut checked, &new, progress)?;
        fs::write(new.join(MANIFEST), &checked.manifest.text)?;
        let _ = writeln!(progress, "building the dictionary");
        let source = new.join("dict-source");
        crate::dictbuild::build(&source, &source.join("dict_extra.tsv"), &new, progress)?;
        let old = current_engine(root);
        // Extra tiers live in the pack folder: carry over the ones the old pack had.
        if let Some(old_pack) = Some(root.join(CURRENT)).filter(|p| p.is_dir()) {
            carry_tiers(&old_pack, &new)?;
        }
        let tier = old.as_ref().map_or("standard", tier_of);
        let (engine, fell_back) = engine_config(&new, old.as_ref(), tier)?;
        write_json(&new.join(ENGINE_CONFIG), &engine)?;
        let _ = writeln!(progress, "self-test");
        self_test(&new, progress)?;
        Ok(Prepared { version: checked.manifest.version.clone(), tier_fell_back: fell_back })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&new);
    }
    result
}

/// Copy the old pack's extra charlm tiers into the new one where the new
/// pack has none of its own (a tier is a model with its own table, apart
/// from the n-gram models).
fn carry_tiers(old: &Path, new: &Path) -> io::Result<()> {
    let (Ok(old_cfg), Ok(new_cfg)) = (read_json(&old.join("config.json")), read_json(&new.join("config.json"))) else {
        return Ok(());
    };
    for tier in ["lite", "advanced"] {
        let (Some(model), None) = (tier_model(old, &old_cfg, tier), tier_model(new, &new_cfg, tier)) else { continue };
        let table = Path::new(&model).with_extension("json");
        for file in [PathBuf::from(&model), table] {
            if let Some(parent) = new.join(&file).parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(old.join(&file), new.join(&file))?;
        }
    }
    Ok(())
}

/// Put ROOT/models-new in place, keeping the pack before as models-previous.
/// Restart the engine afterwards.
pub fn switch(root: &Path) -> io::Result<()> {
    let (current, previous, new) = (root.join(CURRENT), root.join(PREVIOUS), root.join(NEW));
    if !new.join(ENGINE_CONFIG).is_file() {
        return Err(bad("nothing prepared (run `pack prepare` first)"));
    }
    let in_use = |e: io::Error| bad(format!("{e} (is the engine still running? stop it first)"));
    if current.exists() {
        if previous.exists() {
            fs::remove_dir_all(&previous).map_err(in_use)?;
        }
        fs::rename(&current, &previous).map_err(in_use)?;
    }
    if let Err(e) = fs::rename(&new, &current) {
        // Leave things as they were.
        if previous.exists() && !current.exists() {
            let _ = fs::rename(&previous, &current);
        }
        return Err(in_use(e));
    }
    fs::create_dir_all(root.join("userdata"))?;
    Ok(())
}

/// Go back to models-previous; the pack in use becomes models-previous, so
/// rolling back again returns to it.
pub fn rollback(root: &Path) -> io::Result<()> {
    let (current, previous, swap) = (root.join(CURRENT), root.join(PREVIOUS), root.join("models-swap"));
    if !previous.join(ENGINE_CONFIG).is_file() {
        return Err(bad("there is no previous model pack"));
    }
    let in_use = |e: io::Error| bad(format!("{e} (is the engine still running? stop it first)"));
    if swap.exists() {
        fs::remove_dir_all(&swap).map_err(in_use)?;
    }
    fs::rename(&current, &swap).map_err(in_use)?;
    if let Err(e) = fs::rename(&previous, &current) {
        let _ = fs::rename(&swap, &current);
        return Err(in_use(e));
    }
    fs::rename(&swap, &previous).map_err(in_use)
}

/// Add an extra charlm tier (a `bopogram-charlm` pack) to the pack in use.
/// Its files go into models/charlm/; the engine picks it up when the tier is
/// chosen and the engine restarts.
pub fn add_tier(zip_path: &Path, root: &Path, progress: &mut dyn Write) -> io::Result<String> {
    let mut checked = check(zip_path)?;
    if checked.manifest.name != CHARLM_NAME {
        return Err(bad("this is a model pack: use `pack prepare`"));
    }
    let current = root.join(CURRENT);
    if !current.join(ENGINE_CONFIG).is_file() {
        return Err(bad("install a model pack first"));
    }
    for f in &checked.manifest.files {
        let allowed = f.path.starts_with("charlm/charlm-") || matches!(f.path.as_str(), "NOTICE.txt" | "LICENSE-models.txt");
        if !allowed {
            return Err(bad(format!("{}: not part of a charlm tier", f.path)));
        }
    }
    let staging = root.join("tier-new");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    unpack(&mut checked, &staging, progress)?;
    for f in checked.manifest.files.iter().filter(|f| f.path.starts_with("charlm/")) {
        let target = current.join(&f.path);
        fs::create_dir_all(target.parent().unwrap_or(&current))?;
        fs::copy(staging.join(&f.path), &target)?;
    }
    fs::remove_dir_all(&staging)?;
    Ok(checked.manifest.version.clone())
}

/// Choose the charlm tier of the pack in use (takes effect when the engine
/// restarts). Err if the pack does not have that tier.
pub fn set_tier(root: &Path, tier: &str) -> io::Result<()> {
    if !TIERS.contains(&tier) {
        return Err(bad(format!("unknown tier {tier:?} (off, lite, standard, advanced)")));
    }
    let current = root.join(CURRENT);
    let path = current.join(ENGINE_CONFIG);
    let old = read_json(&path)?;
    let (engine, fell_back) = engine_config(&current, Some(&old), tier)?;
    if fell_back {
        return Err(bad(format!("the installed models have no {tier} tier (add its pack first)")));
    }
    write_json(&path, &engine)
}

/// What is installed: the version of the pack in use and before, the tier,
/// and the tiers the pack in use has.
pub fn status(root: &Path) -> Value {
    let version = |dir: &str| {
        read_json(&root.join(dir).join(MANIFEST)).ok().and_then(|m| m.get("version").and_then(Value::as_str).map(str::to_owned))
    };
    let current = root.join(CURRENT);
    let engine = current_engine(root);
    let config = read_json(&current.join("config.json")).ok();
    let tiers: Vec<&str> = TIERS
        .iter()
        .copied()
        .filter(|t| *t == "off" || config.as_ref().is_some_and(|c| tier_model(&current, c, t).is_some()))
        .collect();
    json!({
        "program": engine_version(),
        "current": version(CURRENT),
        "previous": version(PREVIOUS),
        "tier": engine.as_ref().map(tier_of),
        "tiers": tiers,
        "config": engine.as_ref().map(|_| current.join(ENGINE_CONFIG).to_string_lossy().into_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(satisfies(">=0.2", "0.2.0"), Ok(true));
        assert_eq!(satisfies(">=0.2", "0.10.0"), Ok(true));
        assert_eq!(satisfies(">=0.3", "0.2.9"), Ok(false));
        assert!(satisfies("^0.2", "0.2.0").is_err());
    }

    #[test]
    fn paths_inside_the_pack() {
        assert!(safe_relative("models/ly.bglm").is_some());
        assert!(safe_relative("../x").is_none());
        assert!(safe_relative("/etc/passwd").is_none());
        assert!(safe_relative("a\\b").is_none());
        assert!(safe_relative("C:/x").is_none());
    }

    #[test]
    fn tiers_by_model() {
        assert_eq!(tier_of(&json!({"charlm": null})), "off");
        assert_eq!(tier_of(&json!({"charlm": {"model": "charlm/charlm-lite.bin"}})), "lite");
        assert_eq!(tier_of(&json!({"charlm": {"model": "charlm/charlm.bin"}})), "standard");
    }
}
