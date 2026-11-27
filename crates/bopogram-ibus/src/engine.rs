//! The D-Bus side: the factory IBus asks for engines, and the engine objects
//! (one per input context) that turn IBus's calls into controller calls and
//! the controller's state into IBus's signals.
//!
//! The composition is sent with the "commit" preedit mode: when the input
//! context loses focus or is reset, IBus itself puts the shown text into the
//! document. The engine is then told to finish too (so that it learns and
//! keeps the context), and the text it returns is not sent a second time.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bopogram_controller::{Controller, Outcome};
use zbus::object_server::ObjectServer;
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::names::BusName;
use zbus::{interface, Connection};

use crate::keys;
use crate::log::log;
use crate::remote::{Client, Shared};
use crate::values;

const BUS_NAME: &str = "org.freedesktop.IBus.Bopogram";
const FACTORY_PATH: &str = "/org/freedesktop/IBus/Factory";
const ENGINE_IFACE: &str = "org.freedesktop.IBus.Engine";

const PREEDIT_COMMIT: u32 = 1;
const CAP_SURROUNDING_TEXT: u32 = 1 << 5;
const PURPOSE_PASSWORD: u32 = 8;
const PURPOSE_PIN: u32 = 9;

/// Connect to IBus and serve until the bus goes away.
pub fn serve(shared: Arc<Shared>) -> zbus::Result<()> {
    let address = crate::bus::address().ok_or_else(|| zbus::Error::Failure("IBus is not running (no address)".into()))?;
    let factory = Factory { shared, count: 0 };
    let conn = zbus::blocking::connection::Builder::address(address.as_str())?
        .serve_at(FACTORY_PATH, factory)?
        .name(BUS_NAME)?
        .build()?;
    log("connected to IBus");
    for message in zbus::blocking::MessageIterator::from(&conn) {
        if message.is_err() {
            break;
        }
    }
    Ok(())
}

struct Factory {
    shared: Arc<Shared>,
    count: u32,
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl Factory {
    async fn create_engine(
        &mut self,
        name: String,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.count += 1;
        let path = format!("/org/freedesktop/IBus/Engine/Bopogram/{}", self.count);
        log(&format!("create engine {name:?} at {path}"));
        let mut controller = Controller::new(Client::new(Arc::clone(&self.shared)));
        controller.set_options(crate::options());
        let state = Arc::new(Mutex::new(State {
            controller,
            eaten: HashSet::new(),
            surrounding: None,
            capabilities: 0,
            content_type: (0, 0),
            shown: Shown::default(),
            flushing: false,
        }));
        let object_path = OwnedObjectPath::try_from(path.clone()).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let engine = EngineObject { state: Arc::clone(&state), conn: conn.clone(), path: object_path.clone() };
        server.at(path.as_str(), engine).await?;
        server.at(path.as_str(), Service { state, path: object_path.clone() }).await?;
        Ok(object_path)
    }
}

/// What IBus was last told to show, so that only changes are sent.
#[derive(Default, PartialEq, Clone)]
struct Shown {
    preedit: String,
    caret: usize,
    table: Option<(Vec<String>, usize)>,
    aux: Option<String>,
}

struct State {
    controller: Controller<Client>,
    /// Key codes whose press was taken: their release is taken too.
    eaten: HashSet<u32>,
    /// Text before the cursor, when the application reports it.
    surrounding: Option<String>,
    capabilities: u32,
    content_type: (u32, u32),
    shown: Shown,
    /// A thread is sending keys kept while the engine loads.
    flushing: bool,
}

enum Signal {
    Commit(String),
    Preedit(String, usize),
    Table(Option<(Vec<String>, usize, Vec<String>, usize)>),
    Aux(Option<String>),
}

impl State {
    /// The signals that bring IBus up to date after `outcome`.
    fn update(&mut self, outcome: Option<Outcome>, notice: Option<String>) -> Vec<Signal> {
        let mut signals = Vec::new();
        let notice = notice.or_else(|| outcome.as_ref().and_then(|o| o.notice.clone()));
        if let Some(text) = outcome.and_then(|o| o.commit).filter(|t| !t.is_empty()) {
            signals.push(Signal::Commit(text));
        }
        let c = &self.controller;
        let preedit = c.preedit().to_owned();
        let caret = chars_before(c.preedit(), c.caret_units());
        let page = c.candidate_page().map(|(items, selected, page, pages)| {
            let labels: Vec<String> = c.labels().iter().take(items.len()).map(char::to_string).collect();
            (items.to_vec(), selected, labels, page, pages)
        });
        let aux = notice.or_else(|| {
            page.as_ref().and_then(|(_, _, _, page, pages)| (*pages > 1).then(|| format!("{}/{}", page + 1, pages)))
        });
        let table = page.as_ref().map(|(items, selected, _, _, _)| (items.clone(), *selected));
        if preedit != self.shown.preedit || caret != self.shown.caret {
            signals.push(Signal::Preedit(preedit.clone(), caret));
        }
        if table != self.shown.table {
            let page_size = self.controller.labels().len();
            signals.push(Signal::Table(page.map(|(items, selected, labels, _, _)| (items, selected, labels, page_size))));
        }
        if aux != self.shown.aux {
            signals.push(Signal::Aux(aux.clone()));
        }
        self.shown = Shown { preedit, caret, table, aux };
        signals
    }

    /// Forget what is shown (focus left): everything is sent again on return.
    fn hide(&mut self) -> Vec<Signal> {
        self.shown = Shown::default();
        vec![Signal::Preedit(String::new(), 0), Signal::Table(None), Signal::Aux(None)]
    }
}

async fn emit(conn: &Connection, path: &OwnedObjectPath, signals: Vec<Signal>) {
    for signal in signals {
        let result = match signal {
            Signal::Commit(text) => conn.emit_signal(None::<BusName<'_>>, path, ENGINE_IFACE, "CommitText", &(values::text(&text, false),)).await,
            Signal::Preedit(text, caret) => {
                let visible = !text.is_empty();
                let body = (values::text(&text, true), caret as u32, visible, PREEDIT_COMMIT);
                conn.emit_signal(None::<BusName<'_>>, path, ENGINE_IFACE, "UpdatePreeditText", &body).await
            }
            Signal::Table(Some((items, selected, labels, page_size))) => {
                let table = values::lookup_table(&items, &labels, selected, page_size);
                conn.emit_signal(None::<BusName<'_>>, path, ENGINE_IFACE, "UpdateLookupTable", &(table, true)).await
            }
            Signal::Table(None) => conn.emit_signal(None::<BusName<'_>>, path, ENGINE_IFACE, "HideLookupTable", &()).await,
            Signal::Aux(Some(text)) => {
                conn.emit_signal(None::<BusName<'_>>, path, ENGINE_IFACE, "UpdateAuxiliaryText", &(values::text(&text, false), true)).await
            }
            Signal::Aux(None) => conn.emit_signal(None::<BusName<'_>>, path, ENGINE_IFACE, "HideAuxiliaryText", &()).await,
        };
        if let Err(e) = result {
            log(&format!("signal failed: {e}"));
        }
    }
}

struct EngineObject {
    state: Arc<Mutex<State>>,
    conn: Connection,
    path: OwnedObjectPath,
}

impl EngineObject {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn apply(&self, f: impl FnOnce(&mut State) -> Vec<Signal>) {
        let (signals, waiting) = {
            let mut state = self.lock();
            let signals = f(&mut state);
            let waiting = state.controller.waiting() && !state.flushing;
            if waiting {
                state.flushing = true;
            }
            (signals, waiting)
        };
        emit(&self.conn, &self.path, signals).await;
        if waiting {
            self.start_flushing();
        }
    }

    /// Keys typed while the engine loads are kept by the controller: send
    /// them once it is ready, and show the result.
    fn start_flushing(&self) {
        let (state, conn, path) = (Arc::clone(&self.state), self.conn.clone(), self.path.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            let (signals, done) = {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                let commit = s.controller.flush();
                let outcome = Outcome { commit, ..Outcome::default() };
                let signals = s.update(Some(outcome), None);
                let done = !s.controller.waiting();
                if done {
                    s.flushing = false;
                }
                (signals, done)
            };
            zbus::block_on(emit(&conn, &path, signals));
            if done {
                break;
            }
        });
    }

    async fn finish_quietly(&self) {
        self.apply(|s| {
            // IBus commits the shown text itself ("commit" preedit mode).
            let _ = s.controller.finish();
            s.hide()
        })
        .await;
    }
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl EngineObject {
    async fn process_key_event(&mut self, keyval: u32, keycode: u32, state: u32) -> bool {
        let key = keys::key(keyval, state);
        let mut eaten = false;
        self.apply(|s| {
            if keys::is_release(state) {
                eaten = s.eaten.remove(&keycode);
                let chinese = s.controller.chinese();
                let outcome = s.controller.key_up(key);
                let notice = (s.controller.chinese() != chinese).then(|| mode_notice(&s.controller));
                return s.update(Some(outcome), notice);
            }
            if s.controller.starts_composition(key) {
                if let Some(text) = s.surrounding.clone() {
                    s.controller.set_context(&text);
                }
            }
            let outcome = s.controller.key_down(key);
            eaten = outcome.eaten;
            if eaten {
                s.eaten.insert(keycode);
            }
            let notice = outcome.width_changed.then(|| {
                if s.controller.full_width() { "全形".to_owned() } else { "半形".to_owned() }
            });
            let notice = notice.or_else(|| {
                outcome.style_changed.then(|| {
                    if s.controller.style() == bopogram_controller::FORMAL { "正式模式".to_owned() } else { "口語模式".to_owned() }
                })
            });
            s.update(Some(outcome), notice)
        })
        .await;
        eaten
    }

    async fn focus_in(&mut self) {
        let _ = self.conn.emit_signal(None::<BusName<'_>>, &self.path, ENGINE_IFACE, "RequireSurroundingText", &()).await;
        self.apply(|s| s.update(None, None)).await;
    }

    async fn focus_in_id(&mut self, _object_path: String, _client: String) {
        self.focus_in().await;
    }

    async fn focus_out(&mut self) {
        self.finish_quietly().await;
    }

    async fn focus_out_id(&mut self, _object_path: String) {
        self.finish_quietly().await;
    }

    async fn reset(&mut self) {
        self.finish_quietly().await;
    }

    async fn enable(&mut self) {}

    async fn disable(&mut self) {
        self.finish_quietly().await;
    }

    async fn set_cursor_location(&mut self, _x: i32, _y: i32, _w: i32, _h: i32) {}

    async fn set_capabilities(&mut self, caps: u32) {
        let mut s = self.lock();
        s.capabilities = caps;
        if caps & CAP_SURROUNDING_TEXT == 0 {
            s.surrounding = None;
        }
    }

    async fn set_surrounding_text(&mut self, text: Value<'_>, cursor_pos: u32, _anchor_pos: u32) {
        let mut s = self.lock();
        if s.capabilities & CAP_SURROUNDING_TEXT == 0 {
            return;
        }
        s.surrounding = values::text_of(&text).map(|t| t.chars().take(cursor_pos as usize).collect());
    }

    #[zbus(name = "SetContentType")]
    async fn set_content_type_call(&mut self, purpose: u32, hints: u32) {
        self.content_type_changed(purpose, hints).await;
    }

    #[zbus(property(emits_changed_signal = "false"))]
    async fn content_type(&self) -> (u32, u32) {
        self.lock().content_type
    }

    #[zbus(property)]
    async fn set_content_type(&mut self, value: (u32, u32)) {
        self.content_type_changed(value.0, value.1).await;
    }

    async fn candidate_clicked(&mut self, index: u32, _button: u32, _state: u32) {
        self.apply(|s| {
            let at = s.controller.page_start() + index as usize;
            let outcome = s.controller.pick(at);
            s.update(Some(outcome), None)
        })
        .await;
    }

    async fn page_up(&mut self) {
        self.apply(|s| {
            s.controller.turn_page(-1);
            s.update(None, None)
        })
        .await;
    }

    async fn page_down(&mut self) {
        self.apply(|s| {
            s.controller.turn_page(1);
            s.update(None, None)
        })
        .await;
    }

    async fn cursor_up(&mut self) {
        self.apply(|s| {
            if let Some((_, selected, _, _)) = s.controller.candidate_page() {
                let at = (s.controller.page_start() + selected).saturating_sub(1);
                s.controller.highlight(at);
            }
            s.update(None, None)
        })
        .await;
    }

    async fn cursor_down(&mut self) {
        self.apply(|s| {
            if let Some((_, selected, _, _)) = s.controller.candidate_page() {
                let at = s.controller.page_start() + selected + 1;
                s.controller.highlight(at);
            }
            s.update(None, None)
        })
        .await;
    }

    async fn property_activate(&mut self, _name: String, _state: u32) {}

    async fn property_show(&mut self, _name: String) {}

    async fn property_hide(&mut self, _name: String) {}

    async fn process_hand_writing_event(&mut self, _coordinates: Vec<f64>) {}

    async fn cancel_hand_writing(&mut self, _n_strokes: u32) {}
}

impl EngineObject {
    /// Password and PIN fields: nothing is handled, nothing is learned.
    async fn content_type_changed(&self, purpose: u32, hints: u32) {
        self.apply(|s| {
            s.content_type = (purpose, hints);
            let secret = matches!(purpose, PURPOSE_PASSWORD | PURPOSE_PIN);
            s.controller.set_enabled(!secret);
            s.update(None, None)
        })
        .await;
    }
}

fn mode_notice(controller: &Controller<Client>) -> String {
    if controller.chinese() { "中文".to_owned() } else { "英文".to_owned() }
}

/// How many characters of `text` come before `units` UTF-16 units.
fn chars_before(text: &str, units: usize) -> usize {
    let mut seen = 0;
    text.chars()
        .take_while(|c| {
            let before = seen < units;
            seen += c.len_utf16();
            before
        })
        .count()
}

/// `Destroy`, which IBus calls when it no longer needs an engine.
struct Service {
    state: Arc<Mutex<State>>,
    path: OwnedObjectPath,
}

#[interface(name = "org.freedesktop.IBus.Service")]
impl Service {
    async fn destroy(&mut self, #[zbus(object_server)] server: &ObjectServer) {
        log(&format!("destroy {}", self.path.as_str()));
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let _ = s.controller.finish();
        }
        let (server, path) = (server.clone(), self.path.clone());
        // Not from inside this call: the object is in use until it returns.
        std::thread::spawn(move || {
            zbus::block_on(async {
                let _ = server.remove::<EngineObject, _>(&path).await;
                let _ = server.remove::<Service, _>(&path).await;
            })
        });
    }
}
