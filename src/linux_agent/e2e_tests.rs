//! End-to-end tests against a private `ibus-daemon`: the engine registers with it, a test client
//! creates an input context, selects the engine and types; the test checks what IBus relays.
//!
//! The daemon runs with an empty environment and its own directories and address, so it can never
//! reach the user's session bus or replace the IBus of the desktop. Without `ibus-daemon` the tests
//! are skipped, unless `AKL_REQUIRE_IBUS=1` asks for them (the Linux gate and CI do).
//! They type Russian, whose dictionary the modular base downloads, so they need the bundled data.
#![cfg(feature = "legacy-bundled-input")]

use super::{ibus, runtime};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    thread,
    time::{Duration, Instant},
};
use zbus::{
    MatchRule,
    blocking::{Connection, MessageIterator},
    message::Type as MessageType,
    zvariant::{OwnedObjectPath, Value},
};

const CAPABILITIES: u32 = 1 | 8 | 16 | 32; // preedit, focus, property, surrounding text
const RELEASE: u32 = 1 << 30;
const KEY_SPACE: u32 = 57;

struct Daemon {
    child: Child,
    _root: tempfile::TempDir,
    address: String,
    log: PathBuf,
}

impl Daemon {
    /// The GLib warnings the daemon wrote, such as a reply of the engine with the wrong type.
    fn complaints(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter(|line| line.contains("CRITICAL") || line.contains("WARNING"))
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ibus_daemon() -> Option<PathBuf> {
    ["/usr/bin/ibus-daemon", "/usr/local/bin/ibus-daemon"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}

/// Starts a private daemon. With a component file, the daemon reads components from its directory
/// only (the way a user installation points IBus at it); without one, from the system directory.
fn start_daemon(component: Option<&str>) -> Option<Daemon> {
    let Some(program) = ibus_daemon() else {
        assert!(
            std::env::var_os("AKL_REQUIRE_IBUS").is_none(),
            "AKL_REQUIRE_IBUS is set but ibus-daemon is not installed"
        );
        eprintln!("ibus-daemon is not installed; the IBus end-to-end test is skipped");
        return None;
    };
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("ibus.socket");
    let address = format!("unix:path={}", socket.display());
    let directory = |name: &str| -> PathBuf {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    };
    let log = root.path().join("daemon.log");
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(component) = component {
        let components = directory("components");
        std::fs::write(components.join("autokeyboardlayot.xml"), component).unwrap();
        command.env("IBUS_COMPONENT_PATH", components);
    }
    let child = command
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory("home"))
        .env("XDG_CONFIG_HOME", directory("config"))
        .env("XDG_CACHE_HOME", directory("cache"))
        .env("XDG_RUNTIME_DIR", directory("runtime"))
        .env("LANG", "C.UTF-8")
        .args([
            &format!("--address={address}"),
            "--panel=disable",
            "--emoji-extension=disable",
            "--config=disable",
            "--cache=none",
            "--single",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline,
            "ibus-daemon did not create its socket"
        );
        thread::sleep(Duration::from_millis(20));
    }
    Some(Daemon {
        child,
        _root: root,
        address,
        log,
    })
}

/// Runs the engine in a thread of its own, registered with the private daemon.
fn start_engine(address: &str, data: &Path) {
    let (ready, wait) = channel();
    let address = address.to_owned();
    let data = data.to_owned();
    thread::spawn(move || {
        let shared = runtime::load_shared_from(
            &data.join("config"),
            &data.join("data"),
            &data.join("state"),
        )
        .unwrap();
        let connection = ibus::connect(&address).unwrap();
        let _ = ibus::Host::new(shared).serve(&connection, || {
            ibus::register_component(&connection, "/bin/false", "test")?;
            ready.send(()).unwrap();
            Ok(())
        });
    });
    wait.recv_timeout(Duration::from_secs(10))
        .expect("the engine registered");
}

/// Runs the engine the way IBus starts it from a component file: it takes the component's bus name,
/// and the daemon then asks its factory for an engine.
fn start_named_engine(address: &str, data: &Path) {
    let (ready, wait) = channel();
    let address = address.to_owned();
    let data = data.to_owned();
    thread::spawn(move || {
        let shared = runtime::load_shared_from(
            &data.join("config"),
            &data.join("data"),
            &data.join("state"),
        )
        .unwrap();
        let connection = ibus::connect(&address).unwrap();
        let _ = ibus::Host::new(shared).serve(&connection, || {
            connection.request_name(ibus::COMPONENT_NAME)?;
            ready.send(()).unwrap();
            Ok(())
        });
    });
    wait.recv_timeout(Duration::from_secs(10))
        .expect("the engine took its bus name");
}

/// Makes the engine the global engine, as GNOME uses IBus; true when IBus created it.
fn select_engine(connection: &Connection) -> bool {
    connection
        .call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "SetGlobalEngine",
            &(ibus::ENGINE_NAME,),
        )
        .is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Relayed {
    Commit(String),
    Forward { keyval: u32, release: bool },
    Delete { offset: i32, chars: u32 },
    Symbol(String),
}

/// The input-mode symbol in a serialized property list.
fn input_mode_symbol(value: &Value<'_>) -> Option<String> {
    let mut value = value;
    while let Value::Value(inner) = value {
        value = inner;
    }
    let Value::Structure(list) = value else {
        return None;
    };
    let [_, _, Value::Array(properties)] = list.fields() else {
        return None;
    };
    properties.iter().find_map(|property| {
        let mut property = property;
        while let Value::Value(inner) = property {
            property = inner;
        }
        let Value::Structure(property) = property else {
            return None;
        };
        match property.fields() {
            [_, _, Value::Str(key), .., symbol] if key.as_str() == "InputMode" => {
                ibus::text_from_value(symbol)
            }
            _ => None,
        }
    })
}

struct Client {
    connection: Connection,
    context: OwnedObjectPath,
    relayed: Receiver<Relayed>,
}

impl Client {
    fn new(address: &str) -> Self {
        let client = Self::focused(address);
        assert!(
            select_engine(&client.connection),
            "IBus selected the engine"
        );
        client
    }

    /// An input context with the focus, before any engine is selected.
    fn focused(address: &str) -> Self {
        let connection = ibus::connect(address).unwrap();
        let reply = connection
            .call_method(
                Some("org.freedesktop.IBus"),
                "/org/freedesktop/IBus",
                Some("org.freedesktop.IBus"),
                "CreateInputContext",
                &("autokeyboardlayot-test",),
            )
            .unwrap();
        let context: OwnedObjectPath = reply.body().deserialize().unwrap();
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .path(context.clone())
            .unwrap()
            .build();
        let signals = MessageIterator::for_match_rule(rule, &connection, Some(256)).unwrap();
        let (sender, relayed) = channel();
        thread::spawn(move || {
            for message in signals {
                let Ok(message) = message else { break };
                let header = message.header();
                let member = header
                    .member()
                    .map(|name| name.as_str().to_owned())
                    .unwrap_or_default();
                let body = message.body();
                let event = match member.as_str() {
                    "CommitText" => body
                        .deserialize::<(Value<'_>,)>()
                        .ok()
                        .and_then(|(text,)| ibus::text_from_value(&text))
                        .map(Relayed::Commit),
                    "ForwardKeyEvent" => {
                        body.deserialize::<(u32, u32, u32)>()
                            .ok()
                            .map(|(keyval, _, state)| Relayed::Forward {
                                keyval,
                                release: state & RELEASE != 0,
                            })
                    }
                    "DeleteSurroundingText" => body
                        .deserialize::<(i32, u32)>()
                        .ok()
                        .map(|(offset, chars)| Relayed::Delete { offset, chars }),
                    "RegisterProperties" | "UpdateProperty" => body
                        .deserialize::<(Value<'_>,)>()
                        .ok()
                        .and_then(|(list,)| input_mode_symbol(&list))
                        .map(Relayed::Symbol),
                    _ => None,
                };
                if let Some(event) = event
                    && sender.send(event).is_err()
                {
                    break;
                }
            }
        });
        let client = Self {
            connection,
            context,
            relayed,
        };
        client.call("SetCapabilities", &(CAPABILITIES,));
        client.call("FocusIn", &());
        client
    }

    fn call<B>(&self, member: &str, body: &B) -> zbus::message::Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.connection
            .call_method(
                Some("org.freedesktop.IBus"),
                self.context.as_str(),
                Some("org.freedesktop.IBus.InputContext"),
                member,
                body,
            )
            .unwrap()
    }

    /// Presses and releases a key; whether IBus kept the press from the application.
    fn tap(&self, keyval: u32, keycode: u32) -> bool {
        let handled: bool = self
            .call("ProcessKeyEvent", &(keyval, keycode, 0u32))
            .body()
            .deserialize()
            .unwrap();
        let _: bool = self
            .call("ProcessKeyEvent", &(keyval, keycode, RELEASE))
            .body()
            .deserialize()
            .unwrap();
        handled
    }

    /// What IBus relayed until it has been quiet for a moment.
    fn drain(&self) -> Vec<Relayed> {
        let mut events = Vec::new();
        while let Ok(event) = self.relayed.recv_timeout(Duration::from_millis(300)) {
            events.push(event);
        }
        events
    }
}

/// `ghbdtn` in the English layout: the US keysym and the evdev code of each key.
const GHBDTN: [(u32, u32); 6] = [
    (0x67, 34),
    (0x68, 35),
    (0x62, 48),
    (0x64, 32),
    (0x74, 20),
    (0x6e, 49),
];

#[test]
fn a_word_typed_in_the_wrong_layout_is_replaced_through_ibus() {
    let Some(daemon) = start_daemon(None) else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    start_engine(&daemon.address, data.path());
    let client = Client::new(&daemon.address);
    // Selecting the engine registers its properties: the indicator shows EN.
    let events = client.drain();
    assert!(events.contains(&Relayed::Symbol("EN".into())), "{events:?}");
    for (keyval, keycode) in GHBDTN {
        assert!(
            !client.tap(keyval, keycode),
            "the US layout passes keys through"
        );
    }
    assert!(
        client.tap(0x20, KEY_SPACE),
        "the space comes with the replacement"
    );
    let events = client.drain();
    let backspaces = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                Relayed::Forward {
                    keyval: 0xff08,
                    release: false
                }
            )
        })
        .count();
    assert_eq!(backspaces, 6, "{events:?}");
    assert!(
        events.contains(&Relayed::Commit("привет ".into())),
        "{events:?}"
    );
    assert!(events.contains(&Relayed::Symbol("RU".into())), "{events:?}");
    // The engine now types Russian: the key of "d" commits "в".
    assert!(client.tap(0x64, 32));
    assert_eq!(client.drain(), [Relayed::Commit("в".into())]);
    assert_eq!(daemon.complaints(), Vec::<String>::new());
}

#[test]
fn ibus_reads_the_flags_of_the_engine_without_a_complaint() {
    let Some(daemon) = start_daemon(None) else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    start_engine(&daemon.address, data.path());
    let client = Client::new(&daemon.address);
    // IBus asks for the flags when it creates the engine, before the first key reaches it.
    assert!(!client.tap(0x67, 34));
    assert_eq!(daemon.complaints(), Vec::<String>::new());
}

#[test]
fn a_reported_surrounding_text_is_edited_through_ibus() {
    let Some(daemon) = start_daemon(None) else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    start_engine(&daemon.address, data.path());
    let client = Client::new(&daemon.address);
    client.drain();
    for (keyval, keycode) in GHBDTN {
        client.tap(keyval, keycode);
    }
    client.call(
        "SetSurroundingText",
        &(ibus::text_value("ghbdtn"), 6u32, 6u32),
    );
    assert!(client.tap(0x20, KEY_SPACE));
    let events = client.drain();
    assert!(
        events.contains(&Relayed::Delete {
            offset: -6,
            chars: 6
        }),
        "{events:?}"
    );
    assert!(
        events.contains(&Relayed::Commit("привет ".into())),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Relayed::Forward { .. })),
        "{events:?}"
    );
    assert_eq!(daemon.complaints(), Vec::<String>::new());
}

/// A string field of a serialized structure.
fn string_field<'a>(fields: &'a [Value<'a>], index: usize) -> &'a str {
    match &fields[index] {
        Value::Str(text) => text.as_str(),
        other => panic!("field {index} is not a string: {other:?}"),
    }
}

#[test]
fn ibus_daemon_reads_the_component_file() {
    let component = ibus::component_xml("/usr/bin/autokeyboardlayot --ibus", "test");
    let Some(daemon) = start_daemon(Some(&component)) else {
        return;
    };
    let connection = ibus::connect(&daemon.address).unwrap();
    let reply = connection
        .call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "GetEnginesByNames",
            &(vec![ibus::ENGINE_NAME],),
        )
        .unwrap();
    let body = reply.body();
    let (engines,): (Vec<Value<'_>>,) = body.deserialize().unwrap();
    assert_eq!(engines.len(), 1, "{engines:?}");
    let mut engine = &engines[0];
    while let Value::Value(inner) = engine {
        engine = inner;
    }
    let Value::Structure(description) = engine else {
        panic!("not an engine description: {engine:?}");
    };
    let fields = description.fields();
    assert_eq!(string_field(fields, 0), "IBusEngineDesc");
    assert_eq!(string_field(fields, 2), ibus::ENGINE_NAME);
    assert_eq!(string_field(fields, 3), "AutoKeyboardLayot");
    assert_eq!(string_field(fields, 9), "us");
    assert_eq!(string_field(fields, 12), "⌨");
    assert_eq!(string_field(fields, 18), "InputMode");
}

#[test]
fn ibus_creates_an_engine_that_it_started_from_the_component_file() {
    // IBus starts the component's program (here a stand-in that only waits) and creates the engine
    // through the factory of whoever takes the component's bus name: the engine of this test.
    let component = ibus::component_xml("/bin/sleep 10", "test");
    let Some(daemon) = start_daemon(Some(&component)) else {
        return;
    };
    let client = Client::focused(&daemon.address);
    let selecting = {
        let connection = client.connection.clone();
        thread::spawn(move || select_engine(&connection))
    };
    // The daemon has started the program and waits for the name when the engine takes it.
    thread::sleep(Duration::from_millis(300));
    let data = tempfile::tempdir().unwrap();
    start_named_engine(&daemon.address, data.path());
    assert!(selecting.join().unwrap(), "IBus created the engine");
    let events = client.drain();
    assert!(events.contains(&Relayed::Symbol("EN".into())), "{events:?}");
}
