//! End-to-end check of the controller and the pipe client against a real
//! host and engine. Needs the runtime bundle, so it is ignored by default:
//!
//!   set BOPOGRAM_HOST_EXE=<path to bopogram-host.exe, with bopogram-host.json beside it>
//!   cargo test -p bopogram-ime --test host -- --ignored
//!
//! The test starts the host the same way the DLL will: by failing to connect.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use bopogram_ime::controller::{vk, Controller, Key, Outcome};
use bopogram_ime::pipe::PipeEngine;
use bopogram_ime::protocol::{Engine, Op};

fn key(c: char) -> Key {
    let vk = match c {
        ' ' => vk::SPACE,
        c => c.to_ascii_uppercase() as u32,
    };
    Key { vk, ..Key::default() }
}

#[test]
#[ignore = "needs the engine runtime bundle"]
fn type_a_sentence_through_the_host() {
    let host = PathBuf::from(std::env::var_os("BOPOGRAM_HOST_EXE").expect("BOPOGRAM_HOST_EXE"));
    let mut engine = PipeEngine::new(Some(host)).expect("pipe engine");

    // The first attempt starts the host. Its pipe exists before the models
    // are loaded, so keep asking until a request is actually answered.
    let started = Instant::now();
    while engine.request(&Op::State).is_err() {
        assert!(started.elapsed() < Duration::from_secs(30), "host did not come up");
        thread::sleep(Duration::from_millis(100));
    }

    let mut controller = Controller::new(engine);
    controller.set_context("");
    let mut slowest = Duration::ZERO;
    for c in "ji3y94ru8 ".chars() {
        let at = Instant::now();
        assert!(controller.key_down(key(c)).eaten, "key {c:?}");
        slowest = slowest.max(at.elapsed());
    }
    assert_eq!(controller.preedit(), "我在家");

    // Down opens the candidates for the word before the cursor, 在家.
    controller.key_down(Key { vk: vk::DOWN, ..Key::default() });
    let (items, _, _, _) = controller.candidate_page().expect("candidate window");
    assert_eq!(items[0], "在家");
    controller.key_down(Key { vk: vk::ESCAPE, ..Key::default() });
    assert!(controller.candidate_page().is_none());

    // Punctuation joins the composition (Ctrl+comma is the full-width comma).
    controller.key_down(Key { vk: vk::OEM_COMMA, ctrl: true, ..Key::default() });
    assert_eq!(controller.preedit(), "我在家，");
    assert_eq!(controller.caret_units(), 4);
    controller.key_down(Key { vk: vk::LEFT, ..Key::default() });
    assert_eq!(controller.caret_units(), 3);
    controller.key_down(Key { vk: vk::END, ..Key::default() });

    let outcome = controller.key_down(Key { vk: vk::RETURN, ..Key::default() });
    assert_eq!(
        outcome,
        Outcome { eaten: true, commit: Some("我在家，".into()), ..Outcome::default() }
    );
    println!("slowest key: {slowest:?}");
}
